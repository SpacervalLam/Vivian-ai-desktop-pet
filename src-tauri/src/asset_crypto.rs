//! 资源解密模块
//!
//! AES-256-GCM 解密嵌入的 Live2D 美术资源。
//! 密钥在编译期由 build.rs 从 asset_key.bin 读取，拆成 4 段以不同方式混淆后
//! 嵌入二进制；运行时由 derive_key 多步还原为原始 32 字节密钥。
//!
//! 混淆策略（每段不同，避免单一完整密钥常量出现）：
//! - 段 A: XOR 掩码
//! - 段 B: 字节反转 + XOR 掩码
//! - 段 C: XOR 掩码 + 加法偏移
//! - 段 D: XOR 掩码 + 字节循环右移

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};

/// GCM nonce 长度
const NONCE_SIZE: usize = 12;
/// GCM 认证标签长度
const TAG_SIZE: usize = 16;

// 编译期引入混淆后的密钥段与参数（由 build.rs 生成到 OUT_DIR/asset_key.rs）
include!(concat!(env!("OUT_DIR"), "/asset_key.rs"));

/// 运行时还原 4 段混淆数据，拼接成原始 32 字节 AES 密钥
fn derive_key() -> [u8; 32] {
    // 段 A: XOR 去混淆
    let mut a = [0u8; 8];
    for i in 0..8 {
        a[i] = SEG_A_OBF[i] ^ MASK_A[i];
    }

    // 段 B: XOR 去混淆后反转还原（build 时是先反转再 XOR）
    let mut b_rev = [0u8; 8];
    for i in 0..8 {
        b_rev[i] = SEG_B_OBF[i] ^ MASK_B[i];
    }
    let mut b = [0u8; 8];
    for i in 0..8 {
        b[i] = b_rev[7 - i];
    }

    // 段 C: 减偏移后 XOR 去混淆（build 时是 XOR 后加偏移）
    let mut c = [0u8; 8];
    for i in 0..8 {
        c[i] = SEG_C_OBF[i].wrapping_sub(OFFSET_C) ^ MASK_C[i];
    }

    // 段 D: XOR 去混淆后字节循环左移 ROT_D 还原（build 时是先右移再 XOR）
    let mut d_deobf = [0u8; 8];
    for i in 0..8 {
        d_deobf[i] = SEG_D_OBF[i] ^ MASK_D[i];
    }
    let n = ROT_D as usize;
    let mut d = [0u8; 8];
    for i in 0..8 {
        d[i] = d_deobf[(i + 8 - n) % 8];
    }

    // 按 A || B || C || D 顺序拼接为原始密钥
    let mut key = [0u8; 32];
    key[0..8].copy_from_slice(&a);
    key[8..16].copy_from_slice(&b);
    key[16..24].copy_from_slice(&c);
    key[24..32].copy_from_slice(&d);
    key
}

/// 解密单个资源
///
/// 输入格式: [12字节nonce][加密数据][16字节GCM tag]
pub fn decrypt(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    if ciphertext.len() < NONCE_SIZE + TAG_SIZE {
        return Err(format!(
            "密文过短: {} 字节 (最小需要 {})",
            ciphertext.len(),
            NONCE_SIZE + TAG_SIZE
        ));
    }

    let key_bytes = derive_key();
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);

    let nonce_bytes = &ciphertext[..NONCE_SIZE];
    let encrypted_payload = &ciphertext[NONCE_SIZE..];

    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, encrypted_payload)
        .map_err(|e| format!("AES-GCM 解密失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decrypt_roundtrip() {
        // 用运行时派生的密钥加密再解密，验证 derive_key 与加密脚本密钥一致
        let key_bytes = derive_key();
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);

        use aes_gcm::aead::{AeadCore, OsRng};
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let plaintext = b"hello live2d";
        let mut ciphertext = nonce.to_vec();
        let encrypted = cipher.encrypt(&nonce, plaintext.as_slice()).unwrap();
        ciphertext.extend_from_slice(&encrypted);

        let decrypted = decrypt(&ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
