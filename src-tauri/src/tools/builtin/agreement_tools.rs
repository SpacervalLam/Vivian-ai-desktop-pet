//! Bounded maintenance of the collection owned by a standing agreement.
use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest,Sha256};
use crate::memory::{ongoing_tasks::{self, Action, Mode, Source, TaskPatch}, MemoryItem};
use crate::tools::types::{PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult};

pub struct MaintainAgreement;

fn visible_record(item: &MemoryItem) -> Value {
    let mut value = item.metadata["ongoing_task"].clone();
    if let Some(object)=value.as_object_mut() { object.remove("receipts"); }
    json!({"id":item.id,"agreement":value,"collection":{"kind":"agreement_collection","id":item.id}})
}

/// Stored user testimony can seed an initial collection, but assistant summaries cannot.
fn quoted_user_source(item: &MemoryItem, character: &str) -> Option<Source> {
    if !crate::memory::provenance::visible_to(&item.metadata,character)
        || matches!(item.metadata["source"].as_str(),Some("system_seed"|"environment_preset"))
        || item.metadata["is_memory_disabled"]==true || item.metadata["memory_disabled"]==true
        || item.metadata["fictional_backstory"]==true
        || matches!(item.metadata["knowledge_source"].as_str(),Some("heard"|"observed"|"peer_report"))
        || item.metadata["perspective"]=="observer" { return None; }
    let kind=crate::memory::kinds::kind(item);
    let content = if kind==crate::memory::kinds::RecordKind::Dialogue && item.metadata["speaker"]=="user"
        && ongoing_tasks::eligible_source("user",&item.metadata,character) {
        crate::cross_character::parse_any_speaker_prefix(&item.content).0
    } else if kind==crate::memory::kinds::RecordKind::Fact && item.metadata["subject"]=="user"
        && item.metadata["evidence_kind"]=="quoted" {
        item.metadata["source_quote"].as_str()?.to_string()
    } else { return None; };
    Some(Source{id:item.id.clone(),content,timestamp:item.metadata["source_timestamp"].as_f64().unwrap_or(item.timestamp)})
}

#[async_trait]
impl Tool for MaintainAgreement {
    fn name(&self)->&str { "maintain_agreement" }
    fn description(&self)->&str {
        "Read or maintain a standing agreement and its own collection. list/read return scope, status, revision and full entries. create is for an explicit continuing maintenance agreement, never a finite todo. update patches active collections using exact user evidence; planned/in_progress/completed differ. update_once performs an explicitly requested one-time collection edit, even while maintenance is paused/stopped, without re-enabling maintenance; it requires the current user's request_quote, and a progress report alone is not a request. Sources are the current actual user message or source_id memory records returned by search_memory. Supply revision for existing records. pause/resume/stop also require current request_quote. Ordinary progress may be maintained silently. A successful result is the receipt for saying it was saved. Use manage_todo with agreement_id for a concrete finite action. No external file, reminder, browsing or notebook writes are performed."
    }
    fn parameters_schema(&self)->Value { json!({"type":"object","properties":{
        "action":{"type":"string","enum":["list","read","create","update","update_once","pause","resume","stop"]},
        "id":{"type":"string"},"revision":{"type":"integer","minimum":0},
        "key":{"type":"string","description":"Stable semantic key, create only"},
        "title":{"type":"string"},"goal":{"type":"string"},"trigger":{"type":"string"},
        "topics":{"type":"array","items":{"type":"string"}},
        "source_id":{"type":"string","description":"Optional stored user-evidence memory ID; otherwise the host resolves the current user message"},
        "source_quote":{"type":"string","description":"Exact supporting user words"},
        "request_quote":{"type":"string","description":"Current user's explicit request, required for creation/lifecycle changes"},
        "entries":{"type":"array","maxItems":32,"items":{"type":"object","properties":{
            "key":{"type":"string"},"label":{"type":"string"},
            "state":{"type":"string","enum":["planned","in_progress","completed","dropped","recorded"]},
            "detail":{"type":"string"},"source_id":{"type":"string"},"source_quote":{"type":"string"}
        },"required":["key","label","state","source_quote"]}}
    },"required":["action"]}) }
    async fn validate_input(&self,input:&Value,_:&ToolUseContext)->ValidationResult {
        if input["action"].as_str().is_some_and(|a|["list","read","create","update","update_once","pause","resume","stop"].contains(&a)) {
            ValidationResult::success(Some(input.clone()))
        } else { ValidationResult::failure("Unsupported agreement action",2) }
    }
    async fn check_permissions(&self,_:&Value,_:&ToolUseContext)->PermissionResult { PermissionResult::allow() }
    async fn call(&self,args:Value,ctx:&ToolUseContext)->ToolResult {
        let Some(memory)=crate::character_registry::get_memory_manager(&ctx.char_id) else {
            return ToolResult::standard_error("关注与约定不可用",None,None);
        };
        maintain_with_memory(&memory,args,ctx).await
    }
    fn is_read_only(&self)->bool {false}
    fn category(&self)->ToolCategory {ToolCategory::Memory}
    fn risk(&self)->ToolRiskTier {ToolRiskTier::FsWrite}
    fn always_load(&self)->bool {true}
}

async fn maintain_with_memory(memory: &crate::memory::MemoryManager, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let action=args["action"].as_str().unwrap_or("");
        let id=args["id"].as_str().unwrap_or("");
        if action=="list" {
            return ToolResult::standard_success("关注与约定",Some(json!({"agreements":memory.ongoing_task_memories().iter().map(visible_record).collect::<Vec<_>>()})));
        }
        let current=memory.get_memories_by_ids(&[id.to_string()]).pop();
        if action=="read" {
            return match current.filter(|item|ongoing_tasks::task(item).is_some() && item.metadata["memory_owner"]==ctx.char_id) {
                Some(item)=>ToolResult::standard_success("约定及对应清单",Some(visible_record(&item))),
                None=>ToolResult::standard_error("约定已不存在",None,None),
            };
        }
        let all = memory.memories_in_range(ctx.timestamp.timestamp() as f64-1800.0,crate::memory::types::current_timestamp()+1.0);
        let actual = ctx.user_message.as_deref().filter(|text|!text.trim().is_empty()).and_then(|text|all.iter().rev()
            .filter(|item|item.timestamp >= ctx.timestamp.timestamp() as f64-1800.0)
            .filter_map(|item|quoted_user_source(item,&ctx.char_id))
            .filter(|source|source.content.trim()==text.trim()).max_by(|a,b|a.timestamp.total_cmp(&b.timestamp)));
        if action!="update" && actual.is_none() {
            return ToolResult::standard_error("创建或修改约定状态需要当前用户的明确请求",None,None);
        }
        let mut evidence_ids:Vec<String>=args["entries"].as_array().into_iter().flatten().take(32)
            .filter_map(|entry|entry["source_id"].as_str().map(str::to_owned)).collect();
        if let Some(id)=args["source_id"].as_str(){evidence_ids.push(id.into());}
        let mut sources:Vec<_>=memory.get_memories_by_ids(&evidence_ids).iter().filter_map(|item|quoted_user_source(item,&ctx.char_id)).collect();
        if let Some(actual)=&actual {sources.push(actual.clone());}
        let source_id=args["source_id"].as_str().filter(|id|!id.is_empty()).map(str::to_owned)
            .or_else(||actual.as_ref().map(|source|source.id.clone())).unwrap_or_default();
        if action!="update" && actual.as_ref().is_none_or(|source|source.id!=source_id) {
            return ToolResult::standard_error("旧资料不能授权新的约定或范围变更",None,None);
        }
        let mapped=match action {"create"|"resume"=>"activate","pause"=>"pause","stop"=>"cancel","update"=>"update","update_once"=>"update_once",_=>""};
        let mut raw=args.clone();
        raw["action"]=json!(mapped);raw["task_id"]=json!(id);raw["source_id"]=json!(source_id);
        if action=="create" {raw["mode"]=json!("continuous");}
        if let Some(entries)=raw["entries"].as_array_mut() {
            for entry in entries {if entry["source_id"].as_str().is_none_or(str::is_empty) {entry["source_id"]=json!(source_id);}}
        }
        let patch:TaskPatch=match serde_json::from_value(raw) {Ok(p)=>p,Err(e)=>return ToolResult::standard_error("约定参数不完整",Some(&e.to_string()),None)};
        if action=="create" && patch.request_quote.is_empty() {
            return ToolResult::standard_error("持续维护需要明确的 request_quote；单次动作请创建待办",None,None);
        }
        // A finite old record cannot silently become an ongoing agreement through an entry update.
        if patch.action==Action::Update && current.as_ref().and_then(ongoing_tasks::task).is_some_and(|t|t.mode==Mode::Once) {
            return ToolResult::standard_error("这是单次跟进记录，请使用关联待办处理具体动作",None,None);
        }
        let receipt=format!("tool:{}:{}:{:x}",source_id,id,Sha256::digest(serde_json::to_vec(&args).unwrap_or_default()));
        match memory.apply_ongoing_patch(&patch,&sources,&receipt) {
            Ok(Some(id))=>{
                let item=memory.get_memories_by_ids(&[id]).pop().unwrap();
                ToolResult::standard_success("约定及清单已保存",Some(visible_record(&item)))
            },
            Ok(None)=>ToolResult::standard_error("未写入：记录可能已变化、状态不允许维护，或原话依据不匹配；先读取后核对",None,None),
            Err(error)=>ToolResult::standard_error("约定保存失败",Some(&error.to_string()),None),
        }
    }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryManager,MemoryType};
    fn fixture()->(MemoryManager,ToolUseContext) {
        let owner=format!("agreement-tool-{}",uuid::Uuid::new_v4());
        let memory=MemoryManager::new(&crate::config::AppConfig::default(),&owner).unwrap();
        let ctx=ToolUseContext::default().with_char_id(&owner);
        (memory,ctx)
    }
    async fn user(memory:&MemoryManager,text:&str)->MemoryItem {
        memory.add_memory_with_metadata(text,MemoryType::ShortTerm,0.6,vec!["dialogue_turn".into()],
            json!({"record_kind":"dialogue","speaker":"user","listener":memory.char_id(),"channel":"direct"})).await.unwrap()
    }
    #[tokio::test]
    async fn agreement_tool_executes_collection_maintenance_and_paused_scope_blocks_it() {
        let (memory,ctx)=fixture();
        let id=memory.create_agreement("番剧清单","持续维护看番记录","用户提到追番进度").unwrap().unwrap();
        let spoken="我看完迷宫饭了"; user(&memory,spoken).await;
        let ctx=ctx.with_user_message(spoken);
        let args=json!({"action":"update","id":id,"revision":1,"source_quote":"看完迷宫饭了",
            "entries":[{"key":"dungeon","label":"迷宫饭","state":"completed","source_quote":"看完迷宫饭了"}]});
        let result=maintain_with_memory(&memory,args.clone(),&ctx).await;
        assert!(result.success,"{:?}",result.error);
        let item=memory.get_memories_by_ids(&[id.clone()]).pop().unwrap();let task=ongoing_tasks::task(&item).unwrap();
        assert_eq!(task.entries.len(),1);assert_eq!(task.status,ongoing_tasks::Status::Active);
        assert!(task.last_maintenance.is_some());
        assert!(!maintain_with_memory(&memory,args.clone(),&ctx).await.success);
        memory.set_agreement_state(&id,task.revision,ongoing_tasks::Status::Paused).unwrap();
        let mut paused=args;paused["revision"]=json!(task.revision+1);
        assert!(!maintain_with_memory(&memory,paused,&ctx).await.success);
        assert_eq!(ongoing_tasks::task(&memory.get_memories_by_ids(&[id]).pop().unwrap()).unwrap().entries.len(),1);
    }
    #[tokio::test]
    async fn agreement_tool_seeds_prior_quoted_user_memories_but_cannot_reauthorize_from_old_facts() {
        let (memory,ctx)=fixture();
        let id=memory.create_agreement("番剧清单","持续维护看番记录","用户提到追番进度").unwrap().unwrap();
        let fact=memory.add_memory_with_metadata("用户看完了迷宫饭",MemoryType::LongTerm,0.7,vec![],
            json!({"record_kind":"fact","subject":"user","evidence_kind":"quoted","source_quote":"迷宫饭看完了",
                "source_timestamp":1.0,"known_by":[memory.char_id()]})).await.unwrap();
        let result=maintain_with_memory(&memory,json!({"action":"update","id":id,"revision":1,
            "source_id":fact.id,"source_quote":"迷宫饭看完了","entries":[{"key":"dungeon","label":"迷宫饭",
            "state":"completed","source_id":fact.id,"source_quote":"迷宫饭看完了"}]}),&ctx).await;
        assert!(result.success,"{:?}",result.error);
        assert!(!maintain_with_memory(&memory,json!({"action":"resume","id":id,"revision":2,
            "source_id":fact.id,"source_quote":"迷宫饭看完了","request_quote":"迷宫饭看完了"}),&ctx).await.success);
        let mut assistant=fact.clone();assistant.metadata["subject"]=json!("self");
        assert!(quoted_user_source(&assistant,memory.char_id()).is_none());
        let mut seed=fact;seed.metadata["source"]=json!("system_seed");
        assert!(quoted_user_source(&seed,memory.char_id()).is_none());
    }
}
