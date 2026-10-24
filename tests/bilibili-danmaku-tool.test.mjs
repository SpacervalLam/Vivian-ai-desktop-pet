// Compile the real API client, wire parser, tool contract and danmaku tool without Tauri.
// Only the desktop-owned HTTP singleton and image type are supplied by this harness.
import { copyFile, mkdir, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'bilibili-danmaku-tool-tests');
const source = name => path.join(root, 'src-tauri', 'src', name).replaceAll('\\', '/');
await mkdir(path.join(dir, 'src'), { recursive: true });
// Reuse the application's resolved versions, including versions yanked after the lock was made.
await copyFile(path.join(root, 'src-tauri', 'Cargo.lock'), path.join(dir, 'Cargo.lock'));
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-bilibili-danmaku-tool-tests"
version="0.1.0"
edition="2021"
[dependencies]
async-trait="0.1"
serde={version="1",features=["derive"]}
serde_json="1"
chrono={version="0.4",features=["serde"]}
once_cell="1"
regex="1"
parking_lot="0.12"
tracing="0.1"
md-5="0.10"
tokio={version="1",features=["rt-multi-thread","macros","time"]}
reqwest={version="0.12",default-features=false,features=["json","rustls-tls","http2"]}
`);
await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
pub mod brain { pub mod coding_agent { #[derive(Clone)] pub struct CodingImage; } }
pub mod network { pub mod http_client {
    pub fn get_global_client() -> reqwest::Client {
        static CLIENT: once_cell::sync::Lazy<reqwest::Client> = once_cell::sync::Lazy::new(reqwest::Client::new);
        CLIENT.clone()
    }
} }
pub mod discovery {
    #[path="${source('discovery/bilibili.rs')}"] pub mod bilibili;
    #[path="${source('discovery/danmaku.rs')}"] pub mod danmaku;
    pub static BILI_CLIENT: once_cell::sync::Lazy<bilibili::BilibiliClient> = once_cell::sync::Lazy::new(bilibili::BilibiliClient::new);
}
pub mod tools {
    #[path="${source('tools/types.rs')}"] pub mod types;
    #[path="${source('tools/builtin/bilibili_danmaku_tool.rs')}"] pub mod danmaku_tool;
}
#[cfg(test)] mod contracts {
    use super::tools::{danmaku_tool::ReadBilibiliDanmakuTool, types::{Tool, ToolUseContext}};
    #[tokio::test]
    async fn tool_schema_and_validation_use_the_real_tool_trait() {
        let tool = ReadBilibiliDanmakuTool;
        assert_eq!(tool.name(), "read_bilibili_danmaku");
        assert!(tool.is_read_only());
        assert!(tool.should_defer());
        assert_eq!(tool.parameters_schema()["properties"]["limit"]["maximum"], 200);
        let result = tool.call(serde_json::json!({"bvid":"invalid"}), &ToolUseContext::default()).await;
        assert!(!result.success);
        assert!(result.error.is_some());
    }
    #[tokio::test]
    async fn live_read_when_explicitly_requested() {
        let Ok(bvid) = std::env::var("DANMAKU_LIVE_BVID") else { return; };
        let result = ReadBilibiliDanmakuTool.call(serde_json::json!({"bvid":bvid,"limit":3}), &ToolUseContext::default()).await;
        assert!(result.success, "{:?}", result);
        println!("live danmaku receipt: {}", result.data.unwrap());
    }
}
`);
const sample = path.join(root, 'tmp', '_seg.bin');
const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--', '--test-threads=1'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0',
    ...(existsSync(sample) ? { DANMAKU_SEGMENT_FILE: sample } : {}) },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
