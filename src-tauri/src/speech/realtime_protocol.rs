//! 豆包端到端实时语音大模型二进制协议
//!
//! 协议由 4 字节 header + optional 字段 + payload_size(4B) + payload 组成。
//! 详见 https://www.volcengine.com/docs/6561/1594356

use serde_json::Value;

/// 协议版本
const PROTOCOL_VERSION: u8 = 0b0001;
/// Header 大小（4 字节）
const HEADER_SIZE: u8 = 0b0001;

/// 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    /// 客户端文本事件 0b0001
    FullClientRequest = 0b0001,
    /// 服务端文本事件 0b1001
    FullServerResponse = 0b1001,
    /// 客户端音频 0b0010
    AudioOnlyRequest = 0b0010,
    /// 服务端音频 0b1011
    AudioOnlyResponse = 0b1011,
    /// 错误 0b1111
    Error = 0b1111,
}

impl MessageType {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0b0001 => Some(Self::FullClientRequest),
            0b1001 => Some(Self::FullServerResponse),
            0b0010 => Some(Self::AudioOnlyRequest),
            0b1011 => Some(Self::AudioOnlyResponse),
            0b1111 => Some(Self::Error),
            _ => None,
        }
    }
}

/// 序列化方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serialization {
    Raw = 0b0000,
    Json = 0b0001,
}

/// 客户端事件 ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ClientEvent {
    StartConnection = 1,
    FinishConnection = 2,
    StartSession = 100,
    FinishSession = 102,
    TaskRequest = 200,
    SayHello = 300,
    ChatTtsText = 500,
    ChatTextQuery = 501,
    ChatRagText = 502,
}

/// 服务端事件 ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ServerEvent {
    ConnectionStarted = 50,
    ConnectionFailed = 51,
    ConnectionFinished = 52,
    SessionStarted = 150,
    SessionFinished = 152,
    SessionFailed = 153,
    UsageResponse = 154,
    /// ASR 流式识别结果（实测）
    AsrResult = 451,
    /// 对话回合开始（实测）
    DialogRound = 450,
    /// 未知事件
    Unknown = 0,
}

impl ServerEvent {
    pub fn from_u32(id: u32) -> Self {
        match id {
            50 => Self::ConnectionStarted,
            51 => Self::ConnectionFailed,
            52 => Self::ConnectionFinished,
            150 => Self::SessionStarted,
            152 => Self::SessionFinished,
            153 => Self::SessionFailed,
            154 => Self::UsageResponse,
            450 => Self::DialogRound,
            451 => Self::AsrResult,
            _ => Self::Unknown,
        }
    }
}

/// 构建客户端文本事件帧（带 session_id）
pub fn build_client_event_frame(session_id: &str, event: ClientEvent, payload: Value) -> Vec<u8> {
    let payload_str = if payload.is_null() || (payload.is_object() && payload.as_object().map_or(false, |o| o.is_empty())) {
        "{}".to_string()
    } else {
        payload.to_string()
    };
    let payload_bytes = payload_str.as_bytes();
    let sid_bytes = session_id.as_bytes();
    let sid_len = sid_bytes.len() as u32;

    // header(4) + event(4) + session_id_size(4) + session_id + payload_size(4) + payload
    let mut buf = Vec::with_capacity(4 + 4 + 4 + sid_bytes.len() + 4 + payload_bytes.len());
    // header
    buf.push(PROTOCOL_VERSION << 4 | HEADER_SIZE);
    buf.push((MessageType::FullClientRequest as u8) << 4 | 0b0100); // event flag
    buf.push((Serialization::Json as u8) << 4 | 0b0000); // JSON, no compression
    buf.push(0x00); // reserved
    // event id
    buf.extend_from_slice(&(event as u32).to_be_bytes());
    // session id size + session id
    buf.extend_from_slice(&sid_len.to_be_bytes());
    buf.extend_from_slice(sid_bytes);
    // payload size + payload
    buf.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload_bytes);
    buf
}

/// 构建客户端文本事件帧（无 session_id，Connect 类事件）
pub fn build_connect_event_frame(event: ClientEvent, payload: Value) -> Vec<u8> {
    let payload_str = if payload.is_null() || (payload.is_object() && payload.as_object().map_or(false, |o| o.is_empty())) {
        "{}".to_string()
    } else {
        payload.to_string()
    };
    let payload_bytes = payload_str.as_bytes();

    let mut buf = Vec::with_capacity(4 + 4 + 4 + payload_bytes.len());
    // header
    buf.push(PROTOCOL_VERSION << 4 | HEADER_SIZE);
    buf.push((MessageType::FullClientRequest as u8) << 4 | 0b0100);
    buf.push((Serialization::Json as u8) << 4 | 0b0000);
    buf.push(0x00);
    // event id
    buf.extend_from_slice(&(event as u32).to_be_bytes());
    // payload size + payload
    buf.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload_bytes);
    buf
}

/// 构建音频上传帧（TaskRequest，Message Type=0b0010，无序列化）
pub fn build_audio_frame(session_id: &str, pcm_bytes: &[u8]) -> Vec<u8> {
    let sid_bytes = session_id.as_bytes();
    let sid_len = sid_bytes.len() as u32;

    let mut buf = Vec::with_capacity(4 + 4 + 4 + sid_bytes.len() + 4 + pcm_bytes.len());
    // header
    buf.push(PROTOCOL_VERSION << 4 | HEADER_SIZE);
    buf.push((MessageType::AudioOnlyRequest as u8) << 4 | 0b0100); // event flag
    buf.push((Serialization::Raw as u8) << 4 | 0b0000);
    buf.push(0x00);
    // event id (200 = TaskRequest)
    buf.extend_from_slice(&(ClientEvent::TaskRequest as u32).to_be_bytes());
    // session id size + session id
    buf.extend_from_slice(&sid_len.to_be_bytes());
    buf.extend_from_slice(sid_bytes);
    // payload size + payload（直接是音频字节）
    buf.extend_from_slice(&(pcm_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(pcm_bytes);
    buf
}

/// 解析后的服务端帧
#[derive(Debug, Clone)]
pub enum ServerFrame {
    /// 文本事件（JSON payload）
    Text {
        event: ServerEvent,
        event_id: u32,
        session_id: String,
        payload: Value,
    },
    /// 音频帧（PCM 字节）
    Audio {
        session_id: String,
        pcm: Vec<u8>,
    },
    /// 错误帧
    Error {
        code: i32,
        payload: Value,
    },
}

/// 解析服务端二进制帧
pub fn parse_server_frame(data: &[u8]) -> Option<ServerFrame> {
    if data.len() < 4 {
        return None;
    }
    let b0 = data[0];
    let b1 = data[1];
    let _b2 = data[2];
    let _protocol_version = b0 >> 4;
    let _header_size = b0 & 0x0F;
    let msg_type = b1 >> 4;
    let msg_flags = b1 & 0x0F;
    let msg_type = MessageType::from_byte(msg_type)?;

    let mut pos = 4usize;

    // 解析 optional 字段
    let mut code: Option<i32> = None;
    let mut event_id: u32 = 0;
    let mut session_id = String::new();

    if msg_type == MessageType::Error {
        // 错误帧先读 code（4 字节）
        if data.len() < pos + 4 {
            return None;
        }
        code = Some(i32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]));
        pos += 4;
    }

    // sequence（如果 flags 标记）
    let seq_flag = msg_flags & 0b0011;
    if seq_flag != 0b0000 && msg_type == MessageType::AudioOnlyResponse {
        // 跳过 sequence（4 字节）
        if data.len() < pos + 4 {
            return None;
        }
        pos += 4;
    }

    // event（flags 0b0100 表示携带 event id）
    if msg_flags & 0b0100 != 0 {
        if data.len() < pos + 4 {
            return None;
        }
        event_id = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;
    }

    // session id（Session 类事件）
    if matches!(
        msg_type,
        MessageType::FullServerResponse | MessageType::AudioOnlyResponse | MessageType::Error
    ) && msg_flags & 0b0100 != 0
    {
        // session_id_size(4) + session_id
        if data.len() < pos + 4 {
            return None;
        }
        let sid_len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        if data.len() < pos + sid_len {
            return None;
        }
        session_id = String::from_utf8_lossy(&data[pos..pos + sid_len]).to_string();
        pos += sid_len;
    }

    // payload size + payload
    if data.len() < pos + 4 {
        return None;
    }
    let payload_len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
    pos += 4;
    if data.len() < pos + payload_len {
        return None;
    }
    let payload = &data[pos..pos + payload_len];

    match msg_type {
        MessageType::AudioOnlyResponse => Some(ServerFrame::Audio {
            session_id,
            pcm: payload.to_vec(),
        }),
        MessageType::Error => {
            let json: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
            Some(ServerFrame::Error {
                code: code.unwrap_or(0),
                payload: json,
            })
        }
        MessageType::FullServerResponse => {
            let json: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
            Some(ServerFrame::Text {
                event: ServerEvent::from_u32(event_id),
                event_id,
                session_id,
                payload: json,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_connection_frame() {
        let frame = build_connect_event_frame(ClientEvent::StartConnection, serde_json::json!({}));
        // header: [0x11, 0x14, 0x10, 0x00]
        assert_eq!(frame[0], 0x11);
        assert_eq!(frame[1], 0x14);
        assert_eq!(frame[2], 0x10);
        assert_eq!(frame[3], 0x00);
        // event id = 1
        assert_eq!(&frame[4..8], &[0, 0, 0, 1]);
        // payload size = 2
        assert_eq!(&frame[8..12], &[0, 0, 0, 2]);
        // payload = "{}"
        assert_eq!(&frame[12..14], b"{}");
    }
}
