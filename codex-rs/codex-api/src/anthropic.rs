//! DeepSeek / Anthropic-compatible Messages API request types and mapping.
//!
//! dsx talks to DeepSeek's Anthropic-compatible endpoint
//! (`https://api.deepseek.com/anthropic/v1/messages`). The rest of the codebase
//! models a turn as `instructions` + `Vec<ResponseItem>` (the OpenAI Responses
//! shape); this module translates that into an Anthropic Messages request. The
//! streaming response is translated back into the shared [`crate::ResponseEvent`]
//! vocabulary by [`crate::sse::anthropic`].

use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

/// DeepSeek ignores `budget_tokens`, but Anthropic's schema requires it when
/// thinking is enabled, so we send a placeholder.
const THINKING_BUDGET_TOKENS: i64 = 8_192;

/// `max_tokens` is required by the Messages API. DeepSeek caps to the model's
/// real limit server-side; this is a generous default for a coding agent.
// ponytail: one fixed default; move to per-model metadata if a model needs less.
pub const DEFAULT_MAX_OUTPUT_TOKENS: i64 = 32_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MessagesApiRequest {
    pub model: String,
    pub max_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Vec<Value>,
}

/// Inputs for building a [`MessagesApiRequest`] from the internal turn model.
pub struct MessagesRequestParams<'a> {
    pub model: String,
    pub instructions: &'a str,
    pub input: &'a [ResponseItem],
    pub tools: Option<Vec<Value>>,
    pub parallel_tool_calls: bool,
    pub reasoning_enabled: bool,
    pub max_tokens: i64,
}

impl MessagesApiRequest {
    pub fn from_responses(params: MessagesRequestParams<'_>) -> Self {
        let MessagesRequestParams {
            model,
            instructions,
            input,
            tools,
            parallel_tool_calls,
            reasoning_enabled,
            max_tokens,
        } = params;

        let system = (!instructions.trim().is_empty()).then(|| instructions.to_string());
        let messages = messages_from_response_items(input);
        let tool_choice = tools.as_ref().map(|_| {
            json!({
                "type": "auto",
                "disable_parallel_tool_use": !parallel_tool_calls,
            })
        });
        let thinking = reasoning_enabled.then(|| {
            json!({
                "type": "enabled",
                "budget_tokens": THINKING_BUDGET_TOKENS,
            })
        });

        Self {
            model,
            max_tokens,
            system,
            messages,
            tools,
            tool_choice,
            thinking,
            stream: true,
        }
    }
}

/// Translate the internal `ResponseItem` history into Anthropic messages,
/// coalescing consecutive same-role items into one message. Anthropic requires
/// user/assistant alternation with content blocks merged per message; tool
/// calls live in an assistant message and tool results in a user message.
fn messages_from_response_items(items: &[ResponseItem]) -> Vec<AnthropicMessage> {
    let mut messages: Vec<AnthropicMessage> = Vec::new();
    for item in items {
        let Some((role, blocks)) = response_item_to_blocks(item) else {
            continue;
        };
        if blocks.is_empty() {
            continue;
        }
        match messages.last_mut() {
            Some(last) if last.role == role => last.content.extend(blocks),
            _ => messages.push(AnthropicMessage { role, content: blocks }),
        }
    }
    messages
}

fn response_item_to_blocks(item: &ResponseItem) -> Option<(String, Vec<Value>)> {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let anth_role = if role == "assistant" { "assistant" } else { "user" }.to_string();
            let blocks = content.iter().filter_map(content_item_to_block).collect::<Vec<_>>();
            Some((anth_role, blocks))
        }
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        } => {
            let input = serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}));
            Some((
                "assistant".to_string(),
                vec![json!({ "type": "tool_use", "id": call_id, "name": name, "input": input })],
            ))
        }
        ResponseItem::CustomToolCall {
            name,
            input,
            call_id,
            ..
        } => {
            // Freeform/custom tool input is an opaque string; wrap as {"input": ...}
            // when it is not already valid JSON so the Anthropic schema is satisfied.
            let parsed =
                serde_json::from_str::<Value>(input).unwrap_or_else(|_| json!({ "input": input }));
            Some((
                "assistant".to_string(),
                vec![json!({ "type": "tool_use", "id": call_id, "name": name, "input": parsed })],
            ))
        }
        ResponseItem::FunctionCallOutput { call_id, output, .. }
        | ResponseItem::CustomToolCallOutput { call_id, output, .. } => {
            let text = output.body.to_text().unwrap_or_default();
            Some((
                "user".to_string(),
                vec![json!({ "type": "tool_result", "tool_use_id": call_id, "content": text })],
            ))
        }
        // Reasoning is display-only here and not replayed to DeepSeek. Other
        // Responses-API-specific items (web/image/shell/compaction) have no
        // Anthropic equivalent and are dropped.
        // ponytail: revisit if DeepSeek requires prior thinking blocks before tool_use.
        _ => None,
    }
}

fn content_item_to_block(content: &ContentItem) -> Option<Value> {
    match content {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => {
            (!text.is_empty()).then(|| json!({ "type": "text", "text": text }))
        }
        ContentItem::InputImage { image_url, .. } => Some(image_block(image_url)),
    }
}

fn image_block(image_url: &str) -> Value {
    // data:<media-type>;base64,<data>  ->  Anthropic base64 image source.
    if let Some(rest) = image_url.strip_prefix("data:")
        && let Some((meta, data)) = rest.split_once(',')
    {
        let media_type = meta.split(';').next().unwrap_or("image/png").to_string();
        return json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        });
    }
    json!({ "type": "image", "source": { "type": "url", "url": image_url } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::FunctionCallOutputPayload;

    fn user_msg(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText { text: text.to_string() }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    #[test]
    fn coalesces_tool_call_and_result_into_alternating_messages() {
        let items = vec![
            user_msg("hi"),
            ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"cmd\":\"ls\"}".to_string(),
                call_id: "toolu_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: "toolu_1".to_string(),
                output: FunctionCallOutputPayload::from_text("file.txt".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ];
        let msgs = messages_from_response_items(&items);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content[0]["type"], "tool_use");
        assert_eq!(msgs[1].content[0]["id"], "toolu_1");
        assert_eq!(msgs[2].role, "user");
        assert_eq!(msgs[2].content[0]["type"], "tool_result");
        assert_eq!(msgs[2].content[0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn thinking_and_tool_choice_set_when_expected() {
        let input = [user_msg("hi")];
        let req = MessagesApiRequest::from_responses(MessagesRequestParams {
            model: "deepseek-v4-pro".to_string(),
            instructions: "be helpful",
            input: &input,
            tools: Some(vec![json!({"name": "shell"})]),
            parallel_tool_calls: false,
            reasoning_enabled: true,
            max_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
        });
        assert_eq!(req.system.as_deref(), Some("be helpful"));
        assert_eq!(req.thinking.as_ref().unwrap()["type"], "enabled");
        assert_eq!(req.tool_choice.as_ref().unwrap()["disable_parallel_tool_use"], true);
        assert!(req.stream);
    }
}
