pub mod anthropic;
pub mod base;
pub mod capabilities;
pub mod chat_completions;
pub mod doubao;
pub mod factory;
pub mod gemini;
pub mod openai_compat;
pub mod openai_responses;
pub mod reasoning;
pub mod router;
pub mod schema;
pub mod spark;
pub mod thinking_stripper;
pub mod usage_store;
pub mod wenxin;
pub mod zhipu;

// 声明式协议引擎 + JS 插件宿主（provider 全链路插件化的核心）
pub mod declarative;
pub mod js_runtime;
pub mod message_format;
pub mod protocol_registry;
pub mod spec;
pub mod spec_path;
pub mod sse;

pub use declarative::DeclarativeProvider;
pub use js_runtime::{
    JsContribution, JsMcpReg, JsProviderReg, JsRuntime, JsSkillReg, JsToolReg,
};

pub use anthropic::AnthropicProvider;
pub use base::{BaseProvider, ChatResponse, LLMRequest, ProviderBase, ProviderStats, StructuredToolCall, ToolDefinition};
pub use chat_completions::ChatCompletionsProvider;
pub use doubao::DoubaoProvider;
pub use factory::{create_probe_provider, create_task_provider, ClientCache, ProviderKind};
pub use gemini::GeminiProvider;
pub use openai_compat::OpenAiCompatProvider;
pub use openai_responses::OpenAiResponsesProvider;
pub use router::ModelRouter;
pub use schema::{emit_response_tool_definition, is_emit_response_call, validate_vivian_response, vivian_response_schema};
pub use spark::SparkProvider;
pub use thinking_stripper::{leaks_thinking_in_content, strip_thinking_segments, ThinkingStreamStripper};
pub use wenxin::WenxinProvider;
pub use zhipu::ZhipuProvider;
