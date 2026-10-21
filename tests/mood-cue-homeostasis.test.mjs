/**
 * 心情 → 点缀链路的隔离验证（mood_cue / homeostasis / auto_trigger）。
 *
 * 目的：改「情绪怎么衰减」「心情怎么映射成表情」这类逻辑时，不要开整 crate 的
 * cargo check（16GB 机器会 OOM）。这里在 tmp/ 下造一个 mini-crate，用 #[path]
 * 直接挂生产文件，桩掉模型词表 / 动作队列 / 文案查询这几个应用边界，跑真实代码
 * 以及各模块自带的单测。
 *
 * 覆盖：
 *   psychology/{emotion,needs,appraisal,relationship,mood,homeostasis,mood_cue}.rs
 *   engine/auto_trigger.rs
 *
 * 用法：node tests/mood-cue-homeostasis.test.mjs
 * 约 10 秒（依赖已缓存；首次会多花几十秒编依赖）。
 */
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..');
const SRC = path.join(ROOT, 'src-tauri', 'src');
const CRATE = path.join(ROOT, 'tmp', 'mood-cue-homeostasis');

/** 生产模块 → mini-crate 里的挂载路径（用绝对路径避免 Windows 盘符转义） */
const psymods = ['emotion', 'needs', 'appraisal', 'relationship', 'mood', 'homeostasis', 'mood_cue'];

const mount = (mod) => `    #[path = "${path.join(SRC, 'psychology', `${mod}.rs`).replaceAll('\\', '/')}"]
    pub mod ${mod};`;

const lib = `// 由 tests/mood-cue-homeostasis.test.mjs 生成，勿手改。
// 只挂载「心情 -> 点缀」链路的生产模块，其余应用边界用桩替代。

pub mod pipeline {
    pub mod prompt_modules {
        pub fn normalize_lang(lang: &str) -> &str { lang }
        pub fn section_heading(_key: &str, _lang: &str) -> &'static str { "" }
    }
}

pub mod psychology {
${psymods.map(mount).join('\n')}
}

pub mod tools {
    pub mod builtin {
        pub mod pet_tools {
            use std::sync::Mutex;

            #[derive(Debug, Clone)]
            pub struct PetActionRequest {
                pub kind: String,
                pub target: String,
                pub params: serde_json::Value,
                pub timestamp: i64,
                pub character_id: String,
            }

            static QUEUE: Mutex<Vec<PetActionRequest>> = Mutex::new(Vec::new());

            pub fn push_action(req: PetActionRequest) {
                QUEUE.lock().unwrap().push(req);
            }

            pub fn drain_pending_actions(char_id: Option<&str>) -> Vec<PetActionRequest> {
                let mut queue = QUEUE.lock().unwrap();
                let all: Vec<PetActionRequest> = std::mem::take(&mut *queue);
                let (hit, rest): (Vec<_>, Vec<_>) = all.into_iter().partition(|req| match char_id {
                    Some(id) => req.character_id == id,
                    None => true,
                });
                *queue = rest;
                hit
            }
        }
    }
}

pub mod engine {
    pub mod manifest {
        #[derive(Debug, Default, Clone)]
        pub struct ModelManifest;

        impl ModelManifest {
            pub fn load_from_dir(_dir: &std::path::Path) -> Result<Self, String> {
                Err("mini-crate 未挂载模型资源目录".to_string())
            }
        }

        #[derive(Debug, Default, Clone)]
        pub struct ResourceManifest {
            _priv: (),
        }

        impl ResourceManifest {
            pub fn from_manifest(_mf: ModelManifest) -> Self { Self::default() }
            pub fn get_idle_trigger(&self, _key: &str) -> Option<(String, String, Option<u64>)> { None }
            pub fn get_mood_idle_expression(&self, _label: &str) -> Option<(String, u32)> { None }
            pub fn get_event_trigger(&self, _key: &str) -> Option<(String, String, Option<u64>)> { None }
        }
    }

    #[path = "${path.join(SRC, 'engine', 'auto_trigger.rs').replaceAll('\\', '/')}"]
    pub mod auto_trigger;
}
`;

const cargo = `[package]
name = "mood_cue_homeostasis"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rand = "0.9"
tracing = "0.1"
parking_lot = "0.12"
once_cell = "1"
chrono = "0.4"
schemars = { version = "0.8", features = ["preserve_order"] }

[workspace]
`;

fs.mkdirSync(path.join(CRATE, 'src'), { recursive: true });
fs.writeFileSync(path.join(CRATE, 'Cargo.toml'), cargo);
fs.writeFileSync(path.join(CRATE, 'src', 'lib.rs'), lib);

const args = [
  'test', '--offline', '-j', '1', '--',
  // 这条用例需要真实的模型词表目录，归全 crate cargo test 管
  '--skip', 'sleep_idle_trigger_keeps_state_animation_name',
];

const res = spawnSync('cargo', args, {
  cwd: CRATE,
  stdio: 'inherit',
  env: { ...process.env, CARGO_PROFILE_DEV_DEBUG: '0' },
});

if (res.status !== 0) {
  console.error('\n[mood-cue-homeostasis] 失败');
  process.exit(res.status ?? 1);
}
console.log('\n[mood-cue-homeostasis] 通过');
