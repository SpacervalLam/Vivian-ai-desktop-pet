//! Cooperating Vivian processes serialize access to a stateful TTS endpoint.
//! OS file locks are released on drop or process exit; the lock file stays put.

use crate::error::{VivianError, VivianResult};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    time::{Duration, Instant},
};

fn endpoint_key(endpoint: &str) -> VivianResult<String> {
    let mut url = reqwest::Url::parse(endpoint)
        .map_err(|e| VivianError::Speech(format!("TTS 服务地址无效: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(VivianError::Speech("TTS 服务地址必须使用 HTTP(S)".into()));
    }
    if url.host_str() == Some("localhost") {
        let _ = url.set_host(Some("127.0.0.1"));
    }
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_path(&url.path().trim_end_matches('/').to_owned());
    Ok(format!("{:x}", Sha256::digest(url.as_str().as_bytes())))
}

pub struct ModelSession(File);

impl Drop for ModelSession {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub async fn acquire(endpoint: &str, timeout: Duration) -> VivianResult<ModelSession> {
    let key = endpoint_key(endpoint)?;
    let directory = std::env::temp_dir().join("vivian-tts-locks");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|e| VivianError::Speech(format!("创建 TTS 会话目录失败: {e}")))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{key}.lock")))
        .map_err(|e| VivianError::Speech(format!("打开 TTS 会话锁失败: {e}")))?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(ModelSession(file)),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                if Instant::now() >= deadline {
                    return Err(VivianError::Speech("等待 TTS 模型会话超时".into()));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => return Err(VivianError::Speech(format!("获取 TTS 会话锁失败: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_share_a_lock_but_ports_and_paths_do_not() {
        assert_eq!(
            endpoint_key("http://LOCALHOST:9880/").unwrap(),
            endpoint_key("http://127.0.0.1:9880").unwrap()
        );
        assert_ne!(
            endpoint_key("http://127.0.0.1:9880").unwrap(),
            endpoint_key("http://127.0.0.1:9881").unwrap()
        );
        assert_ne!(
            endpoint_key("http://127.0.0.1/a").unwrap(),
            endpoint_key("http://127.0.0.1/b").unwrap()
        );
    }

    #[tokio::test]
    async fn contention_times_out_and_drop_releases() {
        let endpoint = format!("http://localhost/{}", uuid::Uuid::new_v4());
        let first = acquire(&endpoint, Duration::from_secs(1)).await.unwrap();
        assert!(acquire(&endpoint, Duration::from_millis(30)).await.is_err());
        drop(first);
        assert!(acquire(&endpoint, Duration::from_secs(1)).await.is_ok());
    }

    #[tokio::test]
    async fn lock_child_process() {
        let Ok(endpoint) = std::env::var("VIVIAN_TEST_TTS_LOCK_ENDPOINT") else {
            return;
        };
        let _session = acquire(&endpoint, Duration::from_secs(1)).await.unwrap();
        std::fs::write(
            std::env::var("VIVIAN_TEST_TTS_LOCK_READY").unwrap(),
            b"ready",
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    #[tokio::test]
    async fn lock_serializes_separate_processes() {
        let id = uuid::Uuid::new_v4();
        let endpoint = format!("http://localhost/{id}");
        let ready = std::env::temp_dir().join(format!("vivian-lock-test-{id}"));
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "speech::model_session::tests::lock_child_process",
            ])
            .env("VIVIAN_TEST_TTS_LOCK_ENDPOINT", &endpoint)
            .env("VIVIAN_TEST_TTS_LOCK_READY", &ready)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        for _ in 0..200 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready.exists(), "child did not acquire the lock");
        assert!(acquire(&endpoint, Duration::from_millis(30)).await.is_err());
        assert!(child.wait().unwrap().success());
        assert!(acquire(&endpoint, Duration::from_secs(1)).await.is_ok());
        let _ = std::fs::remove_file(ready);
    }
}
