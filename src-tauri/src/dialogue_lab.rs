//! Text-only conversation experiments. No Brain invocation, memory writes or tools.
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabMessage {
    pub role: String,
    pub content: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub character_id: String,
    pub user_input: String,
    pub captured_at: f64,
    pub route: String,
    pub model: String,
    pub messages: Vec<LabMessage>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabTurn {
    pub user: String,
    pub reply: String,
    pub raw: String,
    pub route: String,
    pub model: String,
    pub prompt_note: String,
    pub elapsed_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub snapshot: Snapshot,
    pub turns: Vec<LabTurn>,
}

static LATEST: Lazy<Mutex<HashMap<String, Snapshot>>> = Lazy::new(Default::default);

pub fn capture(snapshot: Snapshot) {
    if snapshot.character_id.is_empty() || snapshot.messages.is_empty() {
        return;
    }
    if serde_json::to_vec(&snapshot).map_or(true, |bytes| bytes.len() > 1_000_000) {
        return;
    }
    LATEST
        .lock()
        .insert(snapshot.character_id.clone(), snapshot);
}
pub fn latest(character_id: &str) -> Option<Snapshot> {
    LATEST.lock().get(character_id).cloned()
}

pub fn branch_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    let canonical = uuid::Uuid::parse_str(id)
        .map_err(|_| "无效的试聊分支 ID")?
        .to_string();
    Ok(root.join(format!("{canonical}.json")))
}
pub fn load(root: &Path, id: &str) -> Result<Branch, String> {
    let path = branch_path(root, id)?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 2_000_000 {
        return Err("试聊分支文件过大".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
pub fn save(root: &Path, branch: &Branch) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let path = branch_path(root, &branch.id)?;
    let bytes = serde_json::to_vec_pretty(branch).map_err(|e| e.to_string())?;
    if bytes.len() > 2_000_000 {
        return Err("试聊分支已达到容量上限".into());
    }
    crate::utils::fs::atomic_write(&path, &bytes).map_err(|e| e.to_string())
}
pub fn fork(source: &Branch, title: String, from_start: bool) -> Branch {
    Branch {
        id: uuid::Uuid::new_v4().to_string(),
        parent_id: Some(source.id.clone()),
        title,
        snapshot: source.snapshot.clone(),
        turns: if from_start {
            Vec::new()
        } else {
            source.turns.clone()
        },
    }
}

/// Keep the frozen post-history direction after each new user turn.
pub fn request_messages(branch: &Branch, input: &str, note: &str) -> Vec<LabMessage> {
    let mut messages = branch.snapshot.messages.clone();
    let insertion = messages
        .iter()
        .rposition(|m| m.role != "system")
        .map_or(messages.len(), |i| i + 1);
    let mut continuation = Vec::new();
    for turn in &branch.turns {
        if !turn.user.is_empty() {
            continuation.push(LabMessage {
                role: "user".into(),
                content: turn.user.clone(),
            });
        }
        if !turn.reply.is_empty() {
            continuation.push(LabMessage {
                role: "assistant".into(),
                content: turn.reply.clone(),
            });
        }
    }
    if !input.trim().is_empty() {
        continuation.push(LabMessage {
            role: "user".into(),
            content: input.trim().into(),
        });
    }
    messages.splice(insertion..insertion, continuation);
    if !note.trim().is_empty() {
        messages.push(LabMessage {
            role: "system".into(),
            content: format!(
                "[USER-AUTHORED EXPERIMENT PREFERENCE]\n{}\n[/USER-AUTHORED EXPERIMENT PREFERENCE]",
                note.trim()
            ),
        });
    }
    messages.push(LabMessage { role: "system".into(), content: "This is an isolated text-only rehearsal. No tools or operations can execute here. Speak only for this character, using the given context. Do not claim to perform new operations. Return one JSON object with text and intent; use empty text and no_reply for silence. No analysis outside JSON.".into() });
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    fn branch() -> Branch {
        Branch {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            title: "A".into(),
            snapshot: Snapshot {
                character_id: "nana".into(),
                user_input: "hello".into(),
                captured_at: 1.0,
                route: "chat".into(),
                model: "example".into(),
                messages: vec![
                    LabMessage {
                        role: "system".into(),
                        content: "frozen persona".into(),
                    },
                    LabMessage {
                        role: "user".into(),
                        content: "hello".into(),
                    },
                    LabMessage {
                        role: "system".into(),
                        content: "post history".into(),
                    },
                ],
            },
            turns: vec![LabTurn {
                user: String::new(),
                reply: "hi".into(),
                raw: "hi".into(),
                route: "chat".into(),
                model: "example".into(),
                prompt_note: String::new(),
                elapsed_ms: 1,
            }],
        }
    }
    #[test]
    fn comparison_fork_reuses_snapshot_without_generated_turns() {
        let source = branch();
        let comparison = fork(&source, "B".into(), true);
        assert!(comparison.turns.is_empty());
        assert_eq!(comparison.snapshot.messages[1].content, "hello");
        assert_ne!(comparison.id, source.id);
        assert_eq!(source.turns.len(), 1);
        assert_eq!(fork(&source, "C".into(), false).turns.len(), 1);
    }
    #[test]
    fn continuation_preserves_role_order_and_frozen_direction() {
        let source = branch();
        let messages = request_messages(&source, "how are you", "gentler");
        assert_eq!(
            messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
            vec![
                "system",
                "user",
                "assistant",
                "user",
                "system",
                "system",
                "system"
            ]
        );
        assert_eq!(messages[4].content, "post history");
        assert_eq!(source.snapshot.messages.len(), 3);
        assert!(!messages
            .last()
            .unwrap()
            .content
            .contains("tools can execute"));
    }
    #[test]
    fn saved_fork_updates_do_not_change_the_source_file() {
        let root =
            std::env::temp_dir().join(format!("vivian-dialogue-test-{}", uuid::Uuid::new_v4()));
        let source = branch();
        save(&root, &source).unwrap();
        let before = std::fs::read(branch_path(&root, &source.id).unwrap()).unwrap();
        let mut variant = fork(&source, "B".into(), true);
        variant.turns.push(LabTurn {
            user: String::new(),
            reply: "another reply".into(),
            raw: "{}".into(),
            route: "reasoning".into(),
            model: "other".into(),
            prompt_note: "gentler".into(),
            elapsed_ms: 2,
        });
        save(&root, &variant).unwrap();
        variant.turns[0].reply = "updated reply".into();
        save(&root, &variant).unwrap();
        assert_eq!(
            load(&root, &variant.id).unwrap().turns[0].reply,
            "updated reply"
        );
        assert_eq!(
            std::fs::read(branch_path(&root, &source.id).unwrap()).unwrap(),
            before
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ids_cannot_escape_the_lab_directory() {
        for id in ["../state", "C:/secret", "", "../../x.json"] {
            assert!(branch_path(Path::new("lab"), id).is_err());
        }
        assert_eq!(
            branch_path(Path::new("lab"), &branch().id)
                .unwrap()
                .parent()
                .unwrap(),
            Path::new("lab")
        );
    }
}
