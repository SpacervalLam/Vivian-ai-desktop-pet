use crate::providers::{base::LLMRequest, router::ModelRouter};
use crate::types::response::{ChatMessage, MessageImage};

/// Both uploaded images and selected screenshots use the same routed recognizer.
pub async fn recognize_image(router: &ModelRouter, image: MessageImage, context: &str, character_id: Option<&str>) -> Result<String, String> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let messages = vec![
        ChatMessage::system(crate::visual_evidence::recognition_prompt(context)),
        ChatMessage::user_with_images(format!("请详细分析这张图片。[req:{}]", &nonce[..8]), vec![image]),
    ];
    let mut request = LLMRequest::new("vision_describe", messages).without_framework_instructions();
    if let Some(id) = character_id { request = request.with_character_id(id.to_string()); }
    let raw = router.generate(request).await.map_err(|e| format!("图片识别失败：{e}"))?;
    crate::visual_evidence::parse_recognition(&raw)
}
