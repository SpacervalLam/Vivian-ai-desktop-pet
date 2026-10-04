// Exercise the current production topic buffer, without model calls.
import { mkdir, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
const root = process.cwd();
const dir = path.join(root, 'tmp', 'topic-judge-tests');
await mkdir(path.join(dir, 'src'), { recursive: true });
await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-topic-judge-tests"
version="0.1.0"
edition="2021"
[dependencies]
parking_lot="0.12"
serde_json="1"
tracing="0.1"
tokio={version="1",features=["macros","rt-multi-thread","time"]}
`);
await writeFile(path.join(dir, 'src/lib.rs'), `#![allow(dead_code)]
pub mod messages { #[derive(Default)] pub struct MessageMeta {pub kind:Option<String>,pub is_memory_disabled:bool} }
pub mod types {pub mod response {
    pub struct ChatMessage {pub role:String,pub content:String,pub tool_calls:Option<()>,pub meta:Option<crate::messages::MessageMeta>}
    impl ChatMessage {
        pub fn user(t:impl Into<String>)->Self{Self::make("user",t)}
        pub fn assistant(t:impl Into<String>)->Self{Self::make("assistant",t)}
        pub fn system(t:impl Into<String>)->Self{Self::make("system",t)}
        pub fn tool_result(t:impl Into<String>,_:&str)->Self{Self::make("tool",t)}
        fn make(r:&str,t:impl Into<String>)->Self{Self{role:r.into(),content:t.into(),tool_calls:None,meta:None}}
    }
}}
#[path="${path.join(root, 'src-tauri/src/brain/topic_signal.rs').replaceAll('\\', '/')}"] pub mod topic_signal;
#[cfg(test)] mod integration {
    use super::topic_signal::TopicSignalBuffer;
    #[test] fn stable_topics_flush_once_and_changes_restabilize() {
        let buffer=TopicSignalBuffer::new();
        assert_eq!(buffer.should_flush("nana"),None);
        for _ in 0..2 {buffer.record_topics("nana",vec!["电影".into()]);assert_eq!(buffer.should_flush("nana"),None);}
        buffer.record_topics("nana",vec!["电影".into()]);
        assert_eq!(buffer.should_flush("nana"),Some(vec!["电影".into()]));
        for _ in 0..4 {buffer.record_topics("nana",vec!["电影".into()]);assert_eq!(buffer.should_flush("nana"),None);}
        for _ in 0..2 {buffer.record_topics("nana",vec!["晚饭".into()]);assert_eq!(buffer.should_flush("nana"),None);}
        buffer.record_topics("nana",vec!["晚饭".into()]);
        assert_eq!(buffer.should_flush("nana"),Some(vec!["晚饭".into()]));
    }
    #[test] fn characters_are_isolated_and_initial_empty_topics_do_not_flush() {
        let buffer=TopicSignalBuffer::new();
        for _ in 0..3 {buffer.record_topics("nana",vec!["电影".into()]);buffer.record_topics("empty",vec![]);}
        buffer.record_topics("vivian",vec!["工作".into()]);
        assert_eq!(buffer.should_flush("vivian"),None);
        assert_eq!(buffer.should_flush("empty"),None);
        assert_eq!(buffer.should_flush("nana"),Some(vec!["电影".into()]));
    }
}
`);
// CARGO_INCREMENTAL=0: rustc 1.96.0 panics in rmeta::encoder when it reuses the
// incremental cache of these generated crates ("the compiler unexpectedly
// panicked"), which made the second and later runs of this harness fail.
const result = spawnSync('cargo', ['test', ...(process.env.VIVIAN_TEST_OFFLINE === '0' ? [] : ['--offline']), '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1'], { stdio: 'inherit', env: { ...process.env, CARGO_INCREMENTAL: '0' } });
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
