use super::*;
use crate::brain::coding_input_queue::InputQueue;

fn validate(payload: &PendingInput) -> Result<(), String> {
    if payload.content.trim().is_empty()
        && payload.images.is_empty()
        && payload.file_refs.is_empty()
    {
        return Err("消息不能为空".into());
    }
    if payload.content.len() > 64 * 1024
        || payload.images.len() > 8
        || payload.file_refs.len() > FILE_REF_MAX_COUNT
    {
        return Err("消息或附件数量超过上限".into());
    }
    if payload
        .images
        .iter()
        .map(|image| image.data.len())
        .sum::<usize>()
        > 8 * 1024 * 1024
    {
        return Err("排队图片合计不能超过 8 MiB".into());
    }
    Ok(())
}

impl CodingAgentService {
    pub fn input_queue(&self, id: &str) -> Result<InputQueue<PendingInput>, String> {
        self.sessions
            .read()
            .get(id)
            .map(|session| session.input_queue.clone())
            .ok_or("会话不存在".into())
    }

    pub fn change_input_queue(
        &self,
        session_id: &str,
        action: &str,
        id: &str,
        payload: PendingInput,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(session_id).ok_or("会话不存在")?;
        match action {
            "add" => {
                if uuid::Uuid::parse_str(id).is_err() {
                    return Err("无效的待发消息 ID".into());
                }
                validate(&payload)?;
                let bytes: usize = session
                    .input_queue
                    .items
                    .iter()
                    .map(|item| {
                        item.payload.content.len()
                            + item
                                .payload
                                .images
                                .iter()
                                .map(|image| image.data.len())
                                .sum::<usize>()
                    })
                    .sum();
                if !session.input_queue.items.iter().any(|item| item.id == id)
                    && !session.input_queue.recent_ids.iter().any(|seen| seen == id)
                    && bytes
                        + payload.content.len()
                        + payload
                            .images
                            .iter()
                            .map(|image| image.data.len())
                            .sum::<usize>()
                        > 16 * 1024 * 1024
                {
                    return Err("待发消息合计不能超过 16 MiB".into());
                }
                session.input_queue.add(id.into(), payload)?;
            }
            "edit" => {
                let mut next = session
                    .input_queue
                    .items
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or("消息已发送或已移除")?
                    .payload
                    .clone();
                next.content = payload.content;
                validate(&next)?;
                session.input_queue.edit(id, |current| *current = next)?;
            }
            "remove" => {
                session.input_queue.remove(id)?;
            }
            "steer" => {
                session.input_queue.steer(id)?;
            }
            "pause" => session.input_queue.pause(true),
            "resume" => session.input_queue.pause(false),
            _ => return Err("未知的待发消息操作".into()),
        }
        session.updated_at = chrono::Utc::now().timestamp_millis();
        drop(sessions);
        self.persist();
        // A successful enqueue acknowledges saved input, not only an in-memory update.
        super::super::coding_io::flush()
    }

    pub fn emit_input_queue(&self, app: &tauri::AppHandle, id: &str) {
        if let Ok(queue) = self.input_queue(id) {
            let _ = app.emit(
                "coding:input_queue",
                serde_json::json!({"session_id":id,"queue":queue}),
            );
        }
    }

    pub fn dispatch_input_queue(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        id: &str,
        router: ModelRouter,
        tools: Arc<ToolSystem>,
        rounds: usize,
    ) {
        let next = {
            let sessions = self.sessions.read();
            sessions
                .get(id)
                .filter(|session| session.status != CodingStatus::Running)
                .and_then(|session| session.input_queue.next())
                .map(|input| input.id.clone())
        };
        if let Some(next) = next {
            if let Err(error) = self.start_message(
                app.clone(),
                id.into(),
                router,
                tools,
                String::new(),
                Vec::new(),
                Vec::new(),
                rounds,
                true,
                false,
                Some(next),
            ) {
                tracing::warn!("Queued input remains pending: {error}");
            }
        }
    }

    pub(super) fn consume_steering(&self, app: &tauri::AppHandle, id: &str) {
        let mut messages = Vec::new();
        {
            let mut sessions = self.sessions.write();
            let Some(session) = sessions
                .get_mut(id)
                .filter(|session| session.status == CodingStatus::Running)
            else {
                return;
            };
            if session
                .task
                .as_ref()
                .is_some_and(|task| task.result.is_some())
            {
                return;
            }
            while let Some(input) = session.input_queue.steering().cloned() {
                let refs = resolve_file_refs(
                    &session.working_directory,
                    &session.extra_workspaces,
                    input.payload.file_refs,
                );
                let message = CodingMessage {
                    role: CodingRole::User,
                    content: input.payload.content,
                    images: (!input.payload.images.is_empty()).then_some(input.payload.images),
                    file_refs: (!refs.is_empty()).then_some(refs),
                    widgets: None,
                    interjected: None,
                    guided: Some(true),
                    tool_name: None,
                    tool_arguments: None,
                    tool_success: None,
                    tool_call_id: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };
                if session.input_queue.remove(&input.id).is_err() {
                    break;
                }
                session.messages.push(message.clone());
                session.updated_at = message.timestamp;
                messages.push((input.id, message));
            }
        }
        if !messages.is_empty() {
            self.persist();
            self.emit_input_queue(app, id);
            for (queued_id, message) in messages {
                super::super::coding_event_log::record(
                    id,
                    None,
                    "steering_delivered",
                    serde_json::json!({"queued_id":queued_id,"content":message.content}),
                );
                let _ = app.emit(
                    "coding:user_message",
                    serde_json::json!({"session_id":id,"queued_id":queued_id,"message":message}),
                );
            }
        }
    }
}
