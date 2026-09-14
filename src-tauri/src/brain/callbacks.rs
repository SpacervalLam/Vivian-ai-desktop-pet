//! 回调系统模块。
//!
//! 提供流式片段与工具调用收集器（`CallbackManager`），供 `chat_chain.rs` / `brain.rs` 使用。

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;

/// 流式片段与工具调用收集器
pub struct CallbackManager {
    stream_chunks: Arc<Mutex<Vec<String>>>,
    tool_calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl CallbackManager {
    pub fn new() -> Self {
        Self {
            stream_chunks: Arc::new(Mutex::new(Vec::new())),
            tool_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn on_stream_chunk(&self, chunk: &str) {
        tracing::debug!(chunk_len = chunk.len(), "收到流式片段");
        self.stream_chunks.lock().push(chunk.to_string());
    }

    pub fn on_tool_call(&self, name: &str, args: &Value) {
        tracing::debug!(tool = %name, args = %args, "工具调用");
        self.tool_calls
            .lock()
            .push((name.to_string(), args.clone()));
    }

    pub fn collected_stream(&self) -> String {
        self.stream_chunks.lock().join("")
    }

    pub fn tool_call_count(&self) -> usize {
        self.tool_calls.lock().len()
    }

    pub fn tool_calls(&self) -> Vec<(String, Value)> {
        self.tool_calls.lock().clone()
    }

    pub fn reset(&self) {
        self.stream_chunks.lock().clear();
        self.tool_calls.lock().clear();
    }
}

impl Default for CallbackManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_callback_manager_stream_and_tool_calls() {
        let mgr = CallbackManager::new();
        mgr.on_stream_chunk("hello ");
        mgr.on_stream_chunk("world");
        assert_eq!(mgr.collected_stream(), "hello world");
        assert_eq!(mgr.tool_call_count(), 0);

        mgr.on_tool_call("search", &serde_json::json!({"q": "test"}));
        assert_eq!(mgr.tool_call_count(), 1);
        mgr.reset();
        assert_eq!(mgr.tool_call_count(), 0);
    }
}
