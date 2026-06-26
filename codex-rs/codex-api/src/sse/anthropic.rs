//! Streaming decoder for DeepSeek's Anthropic-compatible Messages API.
//!
//! Anthropic streams are stateful: content blocks are opened
//! (`content_block_start`), filled with deltas (`content_block_delta`), and
//! closed (`content_block_stop`). We track per-block state and emit the same
//! [`ResponseEvent`]s the Responses-API decoder produces, so the rest of core
//! is wire-agnostic.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

const REQUEST_ID_HEADER: &str = "x-request-id";

pub fn spawn_anthropic_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) -> ResponseStream {
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_anthropic_sse(stream_response.bytes, tx_event, idle_timeout, telemetry).await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

enum BlockKind {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
}

struct BlockState {
    kind: BlockKind,
    buf: String,
}

async fn process_anthropic_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let mut stream = stream.eventsource();
    let mut response_id = String::new();
    let mut input_tokens: i64 = 0;
    let mut cached_input_tokens: i64 = 0;
    let mut output_tokens: i64 = 0;
    let mut stop_reason: Option<String> = None;
    let mut blocks: HashMap<i64, BlockState> = HashMap::new();

    loop {
        let start = Instant::now();
        let next = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&next, start.elapsed());
        }
        let sse = match next {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("anthropic SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before message_stop".into(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("anthropic SSE event: {}", &sse.data);
        if sse.data.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(&sse.data) {
            Ok(v) => v,
            Err(e) => {
                debug!("failed to parse anthropic SSE: {e}, data: {}", &sse.data);
                continue;
            }
        };

        match v.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message_start" => {
                if let Some(msg) = v.get("message") {
                    if let Some(id) = msg.get("id").and_then(Value::as_str) {
                        response_id = id.to_string();
                    }
                    if let Some(usage) = msg.get("usage") {
                        input_tokens = usage.get("input_tokens").and_then(Value::as_i64).unwrap_or(0);
                        cached_input_tokens = usage
                            .get("cache_read_input_tokens")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        output_tokens =
                            usage.get("output_tokens").and_then(Value::as_i64).unwrap_or(0);
                    }
                }
                if tx_event.send(Ok(ResponseEvent::Created)).await.is_err() {
                    return;
                }
            }
            "content_block_start" => {
                let index = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                let cb = v.get("content_block");
                let kind = match cb
                    .and_then(|b| b.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("text")
                {
                    "tool_use" => BlockKind::ToolUse {
                        id: cb
                            .and_then(|b| b.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: cb
                            .and_then(|b| b.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    },
                    "thinking" => BlockKind::Thinking,
                    _ => BlockKind::Text,
                };
                // Establish an active item before any delta arrives: the core
                // turn loop panics on a text/reasoning delta with no active item
                // (and tool blocks need the added event to set up arg streaming).
                let initial_item = match &kind {
                    BlockKind::Text => ResponseItem::Message {
                        id: None,
                        role: "assistant".to_string(),
                        content: Vec::new(),
                        phase: None,
                        internal_chat_message_metadata_passthrough: None,
                    },
                    BlockKind::Thinking => ResponseItem::Reasoning {
                        id: None,
                        summary: Vec::new(),
                        content: Some(Vec::new()),
                        encrypted_content: None,
                        internal_chat_message_metadata_passthrough: None,
                    },
                    BlockKind::ToolUse { id, name } => ResponseItem::FunctionCall {
                        id: None,
                        name: name.clone(),
                        namespace: None,
                        arguments: String::new(),
                        call_id: id.clone(),
                        internal_chat_message_metadata_passthrough: None,
                    },
                };
                blocks.insert(index, BlockState { kind, buf: String::new() });
                if tx_event
                    .send(Ok(ResponseEvent::OutputItemAdded(initial_item)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            "content_block_delta" => {
                let index = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                let Some(delta) = v.get("delta") else { continue };
                match delta.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            if let Some(b) = blocks.get_mut(&index) {
                                b.buf.push_str(text);
                            }
                            if tx_event
                                .send(Ok(ResponseEvent::OutputTextDelta(text.to_string())))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                            if let Some(b) = blocks.get_mut(&index) {
                                b.buf.push_str(text);
                            }
                            if tx_event
                                .send(Ok(ResponseEvent::ReasoningContentDelta {
                                    delta: text.to_string(),
                                    content_index: index,
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            let mut item_id = String::new();
                            if let Some(b) = blocks.get_mut(&index) {
                                b.buf.push_str(partial);
                                if let BlockKind::ToolUse { id, .. } = &b.kind {
                                    item_id = id.clone();
                                }
                            }
                            if tx_event
                                .send(Ok(ResponseEvent::ToolCallInputDelta {
                                    item_id: item_id.clone(),
                                    call_id: Some(item_id),
                                    delta: partial.to_string(),
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    // signature_delta and others carry no displayable content.
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                if let Some(state) = blocks.remove(&index) {
                    let item = match state.kind {
                        BlockKind::Text => ResponseItem::Message {
                            id: None,
                            role: "assistant".to_string(),
                            content: vec![ContentItem::OutputText { text: state.buf }],
                            phase: None,
                            internal_chat_message_metadata_passthrough: None,
                        },
                        BlockKind::Thinking => ResponseItem::Reasoning {
                            id: None,
                            summary: Vec::new(),
                            content: Some(vec![ReasoningItemContent::ReasoningText {
                                text: state.buf,
                            }]),
                            encrypted_content: None,
                            internal_chat_message_metadata_passthrough: None,
                        },
                        BlockKind::ToolUse { id, name } => {
                            let arguments = if state.buf.trim().is_empty() {
                                "{}".to_string()
                            } else {
                                state.buf
                            };
                            ResponseItem::FunctionCall {
                                id: None,
                                name,
                                namespace: None,
                                arguments,
                                call_id: id,
                                internal_chat_message_metadata_passthrough: None,
                            }
                        }
                    };
                    if tx_event
                        .send(Ok(ResponseEvent::OutputItemDone(item)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    stop_reason = Some(reason.to_string());
                }
                if let Some(ot) = v
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_i64)
                {
                    output_tokens = ot;
                }
            }
            "message_stop" => {
                let token_usage = TokenUsage {
                    input_tokens,
                    cached_input_tokens,
                    output_tokens,
                    reasoning_output_tokens: 0,
                    total_tokens: input_tokens + output_tokens,
                };
                let end_turn = stop_reason.as_deref().map(|reason| reason == "end_turn");
                let _ = tx_event
                    .send(Ok(ResponseEvent::Completed {
                        response_id: response_id.clone(),
                        token_usage: Some(token_usage),
                        end_turn,
                    }))
                    .await;
                return;
            }
            "error" => {
                let message = v
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("anthropic stream error")
                    .to_string();
                let _ = tx_event.send(Err(ApiError::Stream(message))).await;
                return;
            }
            "ping" => {}
            other => trace!("unhandled anthropic event: {other}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_client::TransportError;
    use futures::TryStreamExt;
    use tokio_util::io::ReaderStream;

    async fn run(body: &str) -> Vec<ResponseEvent> {
        let stream = ReaderStream::new(std::io::Cursor::new(body.to_string()))
            .map_err(|e| TransportError::Network(e.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
        tokio::spawn(process_anthropic_sse(
            Box::pin(stream),
            tx,
            Duration::from_millis(1000),
            None,
        ));
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev.expect("event"));
        }
        out
    }

    #[tokio::test]
    async fn text_and_tool_call_stream() {
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"shell\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cmd\\\":\\\"ls\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let events = run(body).await;
        assert!(matches!(events[0], ResponseEvent::Created));
        assert!(matches!(&events[1], ResponseEvent::OutputItemAdded(ResponseItem::Message { .. })));
        assert!(matches!(&events[2], ResponseEvent::OutputTextDelta(t) if t == "Hi"));
        assert!(matches!(&events[3], ResponseEvent::OutputItemDone(ResponseItem::Message { .. })));
        assert!(matches!(&events[4], ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall { .. })));
        assert!(matches!(&events[5], ResponseEvent::ToolCallInputDelta { item_id, .. } if item_id == "toolu_9"));
        match &events[6] {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name, arguments, call_id, .. }) => {
                assert_eq!(name, "shell");
                assert_eq!(arguments, "{\"cmd\":\"ls\"}");
                assert_eq!(call_id, "toolu_9");
            }
            other => panic!("expected function call, got {other:?}"),
        }
        match events.last().unwrap() {
            ResponseEvent::Completed { response_id, token_usage, end_turn } => {
                assert_eq!(response_id, "msg_1");
                assert_eq!(token_usage.as_ref().unwrap().output_tokens, 7);
                assert_eq!(*end_turn, Some(false));
            }
            other => panic!("expected completed, got {other:?}"),
        }
    }
}
