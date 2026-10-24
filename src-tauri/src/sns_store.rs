//! Persistent SNS contacts, memberships and canonical group transcripts.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub persona: String,
    #[serde(default)]
    pub model: Option<Value>,
    #[serde(default)]
    pub avatar: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContactInput {
    pub name: String,
    pub persona: String,
    #[serde(default)]
    pub model: Option<Value>,
    #[serde(default)]
    pub avatar: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    pub updated_at: f64,
    pub preview: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupMessage {
    pub id: String,
    pub speaker: String,
    pub role: String,
    pub content: String,
    pub timestamp: f64,
    pub metadata: Value,
}
#[derive(Clone)]
pub struct SnsStore {
    path: PathBuf,
}
pub fn valid_contact_id(id: &str) -> bool {
    id == "deepseek"
        || id
            .strip_prefix("contact_")
            .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
}
pub fn group_channel(id: &str) -> String {
    format!("wechat_group:{id}")
}
pub fn group_id(channel: &str) -> Option<&str> {
    channel
        .strip_prefix("wechat_group:")
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
}
pub fn is_group(channel: &str) -> bool {
    channel == "wechat_group" || group_id(channel).is_some()
}
/// 读人格提示词里的出厂版本标记（`<!-- persona-version: N -->`）。没有标记视为 0。
///
/// 标记只在开头几行找：它是给同步逻辑看的行首元信息，不是人设内容。
pub fn persona_version(persona: &str) -> u32 {
    const MARKER: &str = "persona-version:";
    persona
        .lines()
        .take(8)
        .find_map(|line| {
            // 只认 HTML 注释里的标记：正文里出现同样的词不算版本声明。
            let (head, rest) = line.split_once(MARKER)?;
            if !head.contains("<!--") {
                return None;
            }
            rest.trim_end_matches("-->")
                .split_whitespace()
                .next()?
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })
        .unwrap_or(0)
}
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl SnsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    fn open(&self) -> Result<Connection, String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
        crate::utils::sqlite::configure(&conn, crate::utils::sqlite::DataClass::Authoritative)
            .map_err(|e| e.to_string())?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS contacts(id TEXT PRIMARY KEY,profile TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS groups(id TEXT PRIMARY KEY,name TEXT NOT NULL,created REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS members(group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,contact_id TEXT NOT NULL,PRIMARY KEY(group_id,contact_id));
            CREATE TABLE IF NOT EXISTS messages(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,speaker TEXT NOT NULL,role TEXT NOT NULL,content TEXT NOT NULL,at REAL NOT NULL,metadata TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS group_messages ON messages(group_id,seq DESC);
            CREATE TABLE IF NOT EXISTS state(key TEXT PRIMARY KEY,value TEXT NOT NULL);").map_err(|e|e.to_string())?;
        Ok(conn)
    }
    pub fn seed(&self, contact: &Contact) -> Result<(), String> {
        let mut conn = self.open()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let seeded: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM state WHERE key='contacts_seeded')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !seeded {
            tx.execute(
                "INSERT OR IGNORE INTO contacts VALUES(?1,?2)",
                params![
                    contact.id,
                    serde_json::to_string(contact).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO state VALUES('contacts_seeded','1')", [])
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    /// 只改展示名，其他字段原样写回。幂等：名字一致或联系人不存在时什么都不做。
    /// 用于规范名变更后的一次性同步（`save_contact` 有「创建后不能改名」的守卫，
    /// 内置角色的改名不能走它）。
    pub fn rename_contact(&self, id: &str, name: &str) -> Result<(), String> {
        let mut profile = match self.contact(id) {
            Ok(profile) => profile,
            Err(_) => return Ok(()),
        };
        if profile.name == name {
            return Ok(());
        }
        profile.name = name.to_string();
        self.open()?
            .execute(
                "INSERT INTO contacts(id,profile) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET profile=excluded.profile",
                params![
                    profile.id,
                    serde_json::to_string(&profile).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// 出厂人设随编译期提示词迭代，而 `seed` 只跑一次，老装机需要按版本单向升级。
    ///
    /// 只在库里版本**低于**编译期版本时覆写：上游更新优先，版本相同或更高时原样
    /// 保留，这样用户在「编辑好友」里改过的人设不会被开机同步吃掉。返回是否覆写。
    pub fn sync_persona(&self, id: &str, persona: &str) -> Result<bool, String> {
        let mut profile = match self.contact(id) {
            Ok(profile) => profile,
            Err(_) => return Ok(false),
        };
        if persona_version(&profile.persona) >= persona_version(persona) {
            return Ok(false);
        }
        profile.persona = persona.to_string();
        self.open()?
            .execute(
                "INSERT INTO contacts(id,profile) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET profile=excluded.profile",
                params![
                    profile.id,
                    serde_json::to_string(&profile).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
    pub fn contacts(&self) -> Result<Vec<Contact>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare("SELECT profile FROM contacts ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
    pub fn contact(&self, id: &str) -> Result<Contact, String> {
        self.contacts()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| "好友不存在或已删除".into())
    }
    pub fn save_contact(&self, id: Option<&str>, input: ContactInput) -> Result<Contact, String> {
        let id = match id {
            Some(id) => {
                if !valid_contact_id(id) {
                    return Err("不能修改内置桌宠或无效好友".into());
                }
                self.contact(id)?;
                id.to_string()
            }
            None => format!("contact_{}", uuid::Uuid::new_v4()),
        };
        let name = input.name.trim().to_string();
        let persona = input.persona.trim().to_string();
        if ["vivian", "nana"]
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
            || self
                .contacts()?
                .iter()
                .any(|contact| contact.id != id && contact.name.eq_ignore_ascii_case(&name))
        {
            return Err("这个名字已被其他好友使用".into());
        }

        if let Some(existing_id) = id
            .strip_prefix("contact_")
            .map(|_| id.as_str())
            .or_else(|| (id == "deepseek").then_some(id.as_str()))
        {
            if let Ok(existing) = self.contact(existing_id) {
                if existing.name != name {
                    return Err("好友创建后不能更改名字".into());
                }
            }
        }
        if name.is_empty() || name.chars().count() > 80 || persona.is_empty() {
            return Err("请填写好友名字（最多80字）和人设".into());
        }
        if !input.avatar.is_empty()
            && !input.avatar.starts_with("data:image/")
            && input.avatar != "/icons/charaters/deepseek.webp"
        {
            return Err("头像必须是上传的图片".into());
        }
        if input.avatar.len() > 8 * 1024 * 1024 {
            return Err("头像过大，请使用小于6MB的图片".into());
        }
        if let Some(model) = &input.model {
            if model
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .is_empty()
                || model
                    .get("endpoint")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                return Err("请填写模型名称和接口地址，或选择跟随默认模型".into());
            }
        }
        let profile = Contact {
            id,
            name,
            persona,
            model: input.model,
            avatar: input.avatar,
        };
        self.open()?.execute("INSERT INTO contacts(id,profile) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET profile=excluded.profile",params![profile.id,serde_json::to_string(&profile).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        Ok(profile)
    }
    pub fn delete_contact(&self, id: &str) -> Result<(), String> {
        if !valid_contact_id(id) {
            return Err("Vivian 和 Nana 等内置桌宠不能删除".into());
        }
        let mut conn = self.open()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if tx
            .execute("DELETE FROM contacts WHERE id=?1", [id])
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err("好友不存在".into());
        }
        tx.execute("DELETE FROM members WHERE contact_id=?1", [id])
            .map_err(|e| e.to_string())?;
        // Public speech is a historical snapshot, independent of the contact index.
        // Other participants keep both these messages and their own memories.
        tx.commit().map_err(|e| e.to_string())
    }

    /// Settings shares append fresh identities, so no existing history or memory is reused.
    pub fn append_shared_contacts(&self, inputs: Vec<ContactInput>) -> Result<Vec<Contact>, String> {
        let mut conn = self.open()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut names: HashSet<String> = self.contacts()?.iter()
            .map(|c| c.name.to_lowercase()).chain(["vivian".into(), "nana".into()]).collect();
        let mut profiles = Vec::new();
        for input in inputs {
            let base = input.name.trim();
            if base.is_empty() || base.chars().count() > 80 || input.persona.trim().is_empty()
                || input.avatar.len() > 8 * 1024 * 1024
                || (!input.avatar.is_empty() && !input.avatar.starts_with("data:image/")
                    && input.avatar != "/icons/charaters/deepseek.webp") {
                return Err("分享文件中的好友定义无效".into());
            }
            if let Some(model) = &input.model {
                if !model.is_object() || ["model", "endpoint"].iter().any(|key|
                    model.get(*key).and_then(Value::as_str).unwrap_or("").trim().is_empty()) {
                    return Err("分享文件中的好友模型配置无效".into());
                }
            }
            let mut name = base.to_string();
            let mut suffix = 2;
            while names.contains(&name.to_lowercase()) {
                name = format!("{} ({suffix})", base.chars().take(65).collect::<String>());
                suffix += 1;
            }
            names.insert(name.to_lowercase());
            let profile = Contact { id: format!("contact_{}", uuid::Uuid::new_v4()),
                name, persona: input.persona, model: input.model, avatar: input.avatar };
            tx.execute("INSERT INTO contacts(id,profile) VALUES(?1,?2)",
                params![profile.id, serde_json::to_string(&profile).map_err(|e| e.to_string())?])
                .map_err(|e| e.to_string())?;
            profiles.push(profile);
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(profiles)
    }
    pub fn groups(&self) -> Result<Vec<Group>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare("SELECT id,name,created FROM groups ORDER BY created DESC")
            .map_err(|e| e.to_string())?;
        let base = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        base.into_iter().map(|(id,name,created)|{
            let mut stmt=conn.prepare("SELECT contact_id FROM members WHERE group_id=?1 ORDER BY contact_id").map_err(|e|e.to_string())?;
            let members=stmt.query_map([&id],|r|r.get(0)).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<String>>>().map_err(|e|e.to_string())?;
            let latest:Option<(String,f64)>=conn.query_row("SELECT content,at FROM messages WHERE group_id=?1 ORDER BY seq DESC LIMIT 1",[&id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
            let (preview,updated_at)=latest.unwrap_or((String::new(),created));Ok(Group{id,name,members,updated_at,preview})
        }).collect()
    }
    pub fn group(&self, id: &str) -> Result<Group, String> {
        self.groups()?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or_else(|| "群聊不存在或已解散".into())
    }
    pub fn save_group(
        &self,
        id: Option<&str>,
        name: &str,
        members: Vec<String>,
        allowed: &HashSet<String>,
    ) -> Result<Group, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err("请填写群名称（最多80字）".into());
        }
        let members: std::collections::BTreeSet<_> = members.into_iter().collect();
        if members.is_empty() || members.iter().any(|id| !allowed.contains(id)) {
            return Err("请选择至少一位有效好友加入群聊".into());
        }
        let id = match id {
            Some(id) => {
                self.group(id)?;
                id.to_string()
            }
            None => uuid::Uuid::new_v4().to_string(),
        };
        let mut conn = self.open()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO groups VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET name=excluded.name",
            params![id, name, now()],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM members WHERE group_id=?1", [&id])
            .map_err(|e| e.to_string())?;
        for member in &members {
            tx.execute("INSERT INTO members VALUES(?1,?2)", params![id, member])
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.group(&id)
    }
    pub fn delete_group(&self, id: &str) -> Result<(), String> {
        if self
            .open()?
            .execute("DELETE FROM groups WHERE id=?1", [id])
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err("群聊不存在".into());
        }
        Ok(())
    }
    pub fn append(
        &self,
        group: &str,
        speaker: &str,
        role: &str,
        content: &str,
        metadata: Value,
    ) -> Result<GroupMessage, String> {
        if !matches!(role, "user" | "assistant") {
            return Err("无效消息类型".into());
        }
        let mut metadata = metadata;
        if role == "assistant" {
            if let Ok(contact) = self.contact(speaker) {
                metadata["speaker_name"] = json!(contact.name);
                metadata["speaker_avatar"] = json!(contact.avatar);
            } else if metadata.get("speaker_name").is_none() {
                metadata["speaker_name"] = json!(speaker);
            }
        }
        let message = GroupMessage {
            id: uuid::Uuid::new_v4().to_string(),
            speaker: speaker.into(),
            role: role.into(),
            content: content.into(),
            timestamp: now(),
            metadata,
        };
        let conn = self.open()?;
        let count=conn.execute("INSERT INTO messages(id,group_id,speaker,role,content,at,metadata) SELECT ?1,?2,?3,?4,?5,?6,?7 WHERE EXISTS(SELECT 1 FROM groups WHERE id=?2) AND (?4='user' OR EXISTS(SELECT 1 FROM members WHERE group_id=?2 AND contact_id=?3))",
            params![message.id,group,speaker,role,content,message.timestamp,message.metadata.to_string()]).map_err(|e|e.to_string())?;
        if count == 0 {
            return Err("群已解散或好友已不在群内".into());
        }
        Ok(message)
    }
    pub fn history(
        &self,
        id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GroupMessage>, String> {
        self.group(id)?;
        let conn = self.open()?;
        let cursor = match before {
            Some(cursor_id) => conn
                .query_row(
                    "SELECT seq FROM messages WHERE id=?1 AND group_id=?2",
                    params![cursor_id, id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?
                .unwrap_or(i64::MAX),
            None => i64::MAX,
        };
        let mut stmt=conn.prepare("SELECT id,speaker,role,content,at,metadata FROM messages WHERE group_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT ?3").map_err(|e|e.to_string())?;
        let mut rows = stmt
            .query_map(params![id, cursor, limit.clamp(1, 500) as i64], |r| {
                Ok(GroupMessage {
                    id: r.get(0)?,
                    speaker: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    timestamp: r.get(4)?,
                    metadata: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(json!({})),
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        rows.reverse();
        Ok(rows)
    }
}

/// Only generated contact/group identifiers can name deletion targets.
pub fn owned_directory(root: &Path, id: &str, group: bool) -> Result<PathBuf, String> {
    let valid = if group {
        uuid::Uuid::parse_str(id).is_ok()
    } else {
        valid_contact_id(id)
    };
    if !valid {
        return Err("无效的数据目录标识".into());
    }
    Ok(root
        .join(if group { "sns/groups" } else { "characters" })
        .join(id))
}

pub fn verify_removal(root: &Path, path: &Path) -> Result<(), String> {
    let base = root.canonicalize().map_err(|e| e.to_string())?;
    let target = path.canonicalize().map_err(|e| e.to_string())?;
    if target == base || !target.starts_with(&base) {
        return Err("数据目录超出应用范围".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(name: &str) -> ContactInput {
        ContactInput {
            name: name.into(),
            persona: "A friendly SNS companion".into(),
            model: None,
            avatar: String::new(),
        }
    }
    #[test]
    fn contacts_are_persistent_editable_and_deleted_defaults_do_not_reappear() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnsStore::new(dir.path().join("sns.db"));
        let seed = Contact {
            id: "deepseek".into(),
            name: "鱼".into(),
            persona: "whale".into(),
            model: None,
            avatar: String::new(),
        };
        store.seed(&seed).unwrap();
        let contact = store.save_contact(None, input("Alice")).unwrap();
        assert!(store
            .save_contact(Some(&contact.id), input("Alicia"))
            .is_err());
        let mut edit = input("Alice");
        edit.persona = "updated personality".into();
        store.save_contact(Some(&contact.id), edit).unwrap();
        assert_eq!(store.contact(&contact.id).unwrap().name, "Alice");
        assert!(store.delete_contact("vivian").is_err());
        assert!(store.delete_contact("nana").is_err());
        store.delete_contact("deepseek").unwrap();
        store.seed(&seed).unwrap();
        assert!(store.contact("deepseek").is_err());
        assert_eq!(
            SnsStore::new(dir.path().join("sns.db"))
                .contacts()
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn groups_are_isolated_paginated_and_deletion_cascades() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnsStore::new(dir.path().join("sns.db"));
        let a = store.save_contact(None, input("A")).unwrap();
        let allowed = HashSet::from([a.id.clone(), "vivian".into(), "nana".into()]);
        let g1 = store
            .save_group(None, "one", vec![a.id.clone(), "vivian".into()], &allowed)
            .unwrap();
        let g2 = store
            .save_group(None, "two", vec!["nana".into()], &allowed)
            .unwrap();
        assert!(store
            .append(&g1.id, "nana", "assistant", "secret", json!({}))
            .is_err());
        let first = store
            .append(&g1.id, "user", "user", "private to group one", json!({}))
            .unwrap();
        let second = store
            .append(&g1.id, &a.id, "assistant", "reply", json!({}))
            .unwrap();
        assert!(store.history(&g2.id, None, 20).unwrap().is_empty());
        assert_eq!(
            store.history(&g1.id, Some(&second.id), 20).unwrap()[0].id,
            first.id
        );
        store.delete_contact(&a.id).unwrap();
        assert_eq!(store.history(&g1.id, None, 20).unwrap().len(), 2);
        assert_eq!(
            store.history(&g1.id, None, 20).unwrap()[1].metadata["speaker_name"],
            "A"
        );
        assert!(store.contact(&a.id).is_err());
        assert_eq!(store.group(&g1.id).unwrap().members, vec!["vivian"]);
        store.delete_group(&g1.id).unwrap();
        assert!(store
            .append(&g1.id, "user", "user", "late", json!({}))
            .is_err());
        assert!(store.history(&g1.id, None, 20).is_err());
        assert_eq!(store.groups().unwrap().len(), 1);
    }
    #[test]
    fn deletion_paths_cannot_escape_or_target_protected_pets() {
        let root = Path::new("/private/data");
        for id in ["../vivian", "vivian", "nana", "", "C:/"] {
            assert!(owned_directory(root, id, false).is_err());
        }
        assert!(owned_directory(root, "deepseek", false)
            .unwrap()
            .ends_with("characters/deepseek"));
        assert!(group_id("wechat_group:../x").is_none());
    }
    #[test]
    fn deleting_contact_preserves_survivors_memories_and_historical_identity() {
        use crate::chat_partner_memory::{SemanticMemory, Snippet};
        let dir = tempfile::tempdir().unwrap();
        let store = SnsStore::new(dir.path().join("sns.db"));
        let departed = store.save_contact(None, input("Whale")).unwrap();
        let survivor = store.save_contact(None, input("Friend")).unwrap();
        let allowed = HashSet::from([departed.id.clone(), survivor.id.clone()]);
        let group = store
            .save_group(None, "Friends", allowed.iter().cloned().collect(), &allowed)
            .unwrap();
        let message = store
            .append(
                &group.id,
                &departed.id,
                "assistant",
                "鲸鱼喜欢热可可",
                json!({}),
            )
            .unwrap();
        let memory = SemanticMemory::new(dir.path().join("survivor.db"));
        let channel = group_channel(&group.id);
        memory
            .remember(
                &channel,
                &[Snippet {
                    source_id: message.id,
                    channel: channel.clone(),
                    speaker: departed.id.clone(),
                    content: message.content,
                    timestamp: message.timestamp,
                }],
            )
            .unwrap();
        store.delete_contact(&departed.id).unwrap();
        assert_eq!(
            memory.recall(&channel, "热可可", &[], None).unwrap()[0].speaker,
            departed.id
        );
        assert_eq!(
            store.history(&group.id, None, 20).unwrap()[0].metadata["speaker_name"],
            "Whale"
        );
        assert!(store
            .append(
                &group.id,
                &departed.id,
                "assistant",
                "late reply",
                json!({})
            )
            .is_err());
        // A new friend with the same name gets a different identity; old messages never rebind.
        let replacement = store.save_contact(None, input("Whale")).unwrap();
        assert_ne!(replacement.id, departed.id);
        assert_eq!(
            store.history(&group.id, None, 20).unwrap()[0].speaker,
            departed.id
        );
        assert_eq!(store.group(&group.id).unwrap().members, vec![survivor.id]);
    }
    #[test]
    fn models_are_independent_and_names_are_create_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnsStore::new(dir.path().join("sns.db"));
        let mut first = input("Alice");
        first.model = Some(json!({"model":"one","endpoint":"http://localhost:1234/v1"}));
        let a = store.save_contact(None, first.clone()).unwrap();
        let b = store.save_contact(None, input("Bob")).unwrap();
        assert!(store.save_contact(Some(&a.id), input("Bob")).is_err());
        assert!(store.save_contact(None, input("Alice")).is_err());
        first.persona = "New personality".into();
        first.model = Some(json!({"model":"two","endpoint":"http://localhost:1234/v1"}));
        let edited = store.save_contact(Some(&a.id), first).unwrap();
        assert_eq!(edited.name, "Alice");
        assert_eq!(edited.model.unwrap()["model"], "two");
        assert!(store.contact(&b.id).unwrap().model.is_none());
    }
}
