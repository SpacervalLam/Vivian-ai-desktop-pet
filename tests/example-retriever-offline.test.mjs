// Compile the production example_retriever (with its own unit tests) without the desktop runtime.
// Verifies the offline lexical-fallback channel: paraphrase recovery, literal priority,
// single-fallback cap, thin-input rejection, continuation threshold, score band.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const dir = path.join(root, 'tmp', 'example-retriever-tests');
const persona = path.join(root, 'src-tauri', 'src', 'persona');
const messageContext = path.join(root, 'src-tauri', 'src', 'pipeline', 'message_context.rs');
await mkdir(path.join(dir, 'src'), { recursive: true });
const rustPath = value => value.replaceAll('\\', '/');

// is_short_followup is self-contained (no crate deps): extract it verbatim from production
// so the continuation path exercises the real predicate, not a stub approximation.
const source = (await readFile(messageContext, 'utf8')).replaceAll('\r\n', '\n');
const start = source.indexOf('pub fn is_short_followup');
const end = source.indexOf('\n}\n', start) + 3;
if (start < 0 || end < 3) throw new Error('is_short_followup slice not found');
const followup = source.slice(start, end);

await writeFile(path.join(dir, 'Cargo.toml'), `[package]
name="vivian-example-retriever-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde={version="1",features=["derive"]}
serde_json="1"
parking_lot="0.12"
tracing="0.1"
`);

await writeFile(path.join(dir, 'src', 'lib.rs'), `#![allow(dead_code)]
pub mod types { pub mod response {
    #[derive(Clone, Debug, serde::Deserialize)]
    pub struct ChatMessage { pub role: String, pub content: String }
}}
pub mod memory {
    pub mod embedding {
        use std::sync::Arc;
        pub type VivianResult<T> = Result<T, String>;
        pub trait MemoryEmbeddingProvider: Send + Sync {
            fn dimension(&self) -> usize;
            fn embed(&self, text: &str) -> VivianResult<Vec<f32>>;
            fn embed_batch(&self, texts: &[String]) -> VivianResult<Vec<Vec<f32>>> {
                texts.iter().map(|t| self.embed(t)).collect()
            }
            fn is_remote(&self) -> bool { false }
        }
        pub struct HashEmbedding;
        impl MemoryEmbeddingProvider for HashEmbedding {
            fn dimension(&self) -> usize { 64 }
            fn embed(&self, _text: &str) -> VivianResult<Vec<f32>> { Ok(vec![0.0; 64]) }
        }
        pub fn default_embedding() -> Arc<dyn MemoryEmbeddingProvider> { Arc::new(HashEmbedding) }
    }
    pub mod vector_search {
        pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
            let n = a.len().min(b.len());
            if n == 0 { return 0.0; }
            let (mut dot, mut na, mut nb) = (0.0_f64, 0.0_f64, 0.0_f64);
            for i in 0..n { dot += a[i] as f64 * b[i] as f64; na += (a[i] as f64).powi(2); nb += (b[i] as f64).powi(2); }
            if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
        }
    }
}
pub mod error { pub type VivianResult<T> = Result<T, String>; }
// Compilation-only stubs for one pre-existing integration test
// (modern_net_examples_reach_companion_prompt_without_remote_embeddings) that
// exercises the real CompanionPrompt; it is skipped below because a stub here
// could only test the stub. Full-crate cargo test covers it.
pub mod pipeline { pub mod prompt_modules {
    #[derive(Default)]
    pub struct PromptParts {
        pub char_id: String,
        pub channel: String,
        pub examples_block: Option<String>,
    }
}
pub mod companion_prompt {
    use crate::types::response::ChatMessage;
    #[derive(Default)]
    pub struct CompanionPrompt { examples: Option<String> }
    impl CompanionPrompt {
        pub fn build(parts: &crate::pipeline::prompt_modules::PromptParts, _history: &[ChatMessage]) -> Self {
            Self { examples: parts.examples_block.clone() }
        }
        pub fn messages(&self, _history: &[ChatMessage], _input: &str, _internal: bool, _status: Option<&str>) -> Vec<ChatMessage> {
            match &self.examples {
                Some(examples) => vec![ChatMessage { role: "system".into(), content: examples.clone() }],
                None => Vec::new(),
            }
        }
    }
}
pub mod message_context {
${followup}
}}
#[path="${rustPath(path.join(persona, 'example_retriever.rs'))}"] pub mod example_retriever;
`);

const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', path.join(dir, 'Cargo.toml'), '-j', '1', '--',
  '--test-threads=1', '--skip', 'modern_net_examples_reach_companion_prompt_without_remote_embeddings'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_INCREMENTAL: '0', CARGO_PROFILE_DEV_DEBUG: '0' },
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
