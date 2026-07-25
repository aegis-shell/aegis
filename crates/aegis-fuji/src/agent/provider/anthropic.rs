//! Anthropic Messages API (`POST /v1/messages`) with SSE streaming.

use futures::StreamExt;
use serde_json::{Value, json};

use super::sse::{SseEvent, SseParser};
use super::{
    ContentBlock, ImageData, Message, Provider, ProviderError, Request, Response, Role, StopReason,
    StreamEvent, Usage,
};

const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_key: api_key.into(),
        }
    }
}

impl Provider for AnthropicProvider {
    async fn stream(
        &self,
        request: &Request,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<Response, ProviderError> {
        let response = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&request_body(request))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                body: super::capped(&body),
            });
        }
        let mut bytes = response.bytes_stream();
        let mut parser = SseParser::new();
        let mut assembler = AnthropicAssembler::default();
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk?;
            for event in parser.feed(&chunk) {
                assembler.process(&event, on_event)?;
            }
        }
        for event in parser.finish() {
            assembler.process(&event, on_event)?;
        }
        assembler.finish(on_event)
    }
}

/// Serialize the shared request into Anthropic's wire shape.
fn request_body(request: &Request) -> Value {
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "messages": request.messages.iter().map(wire_message).collect::<Vec<_>>(),
        "stream": true,
    });
    if let Some(system) = &request.system {
        body["system"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                })
            })
            .collect();
    }
    body
}

fn wire_message(message: &Message) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content = message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => json!({"type": "text", "text": text}),
            ContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult {
                tool_use_id,
                text,
                image,
                is_error,
            } => {
                let mut parts = Vec::new();
                if !text.is_empty() {
                    parts.push(json!({"type": "text", "text": text}));
                }
                if let Some(image) = image {
                    parts.push(image_content(image));
                }
                json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "is_error": is_error,
                    "content": parts,
                })
            }
            ContentBlock::Image(image) => image_content(image),
        })
        .collect::<Vec<_>>();
    json!({"role": role, "content": content})
}

fn image_content(image: &ImageData) -> Value {
    json!({
        "type": "image",
        "source": {"type": "base64", "media_type": image.media_type, "data": image.data},
    })
}

/// Folds SSE events into stream callbacks and the final [`Response`].
/// Kept HTTP-free so recorded streams can drive it in tests.
#[derive(Default)]
pub(crate) struct AnthropicAssembler {
    blocks: Vec<ContentBlock>,
    current_text: Option<String>,
    current_tool: Option<(String, String, String)>,
    stop_reason: Option<StopReason>,
    usage: Usage,
}

impl AnthropicAssembler {
    pub(crate) fn process(
        &mut self,
        event: &SseEvent,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<(), ProviderError> {
        match event.event.as_deref().unwrap_or("") {
            "message_start" => {
                let value: Value = serde_json::from_str(&event.data)?;
                if let Some(tokens) = value["message"]["usage"]["input_tokens"].as_u64() {
                    self.usage.input_tokens = tokens;
                }
            }
            "content_block_start" => {
                let value: Value = serde_json::from_str(&event.data)?;
                match value["content_block"]["type"].as_str() {
                    Some("text") => self.current_text = Some(String::new()),
                    Some("tool_use") => {
                        let id = value["content_block"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let name = value["content_block"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        self.current_tool = Some((id.clone(), name.clone(), String::new()));
                        on_event(StreamEvent::ToolUseStart { id, name });
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let value: Value = serde_json::from_str(&event.data)?;
                match value["delta"]["type"].as_str() {
                    Some("text_delta") => {
                        let text = value["delta"]["text"].as_str().unwrap_or_default();
                        self.current_text
                            .get_or_insert_with(String::new)
                            .push_str(text);
                        on_event(StreamEvent::TextDelta(text.to_string()));
                    }
                    Some("input_json_delta") => {
                        let partial = value["delta"]["partial_json"].as_str().unwrap_or_default();
                        if let Some((_, _, buffer)) = &mut self.current_tool {
                            buffer.push_str(partial);
                        }
                        on_event(StreamEvent::ToolUseInputDelta(partial.to_string()));
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some((id, name, buffer)) = self.current_tool.take() {
                    let input = if buffer.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&buffer)?
                    };
                    self.blocks.push(ContentBlock::ToolUse { id, name, input });
                    on_event(StreamEvent::ToolUseEnd);
                } else if let Some(text) = self.current_text.take() {
                    self.blocks.push(ContentBlock::Text(text));
                }
            }
            "message_delta" => {
                let value: Value = serde_json::from_str(&event.data)?;
                self.stop_reason = Some(super::stop_reason(value["delta"]["stop_reason"].as_str()));
                if let Some(tokens) = value["usage"]["output_tokens"].as_u64() {
                    self.usage.output_tokens = tokens;
                }
            }
            "message_stop" => on_event(StreamEvent::Done),
            "error" => {
                let value: Value = serde_json::from_str(&event.data)?;
                let message = value["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown stream error");
                return Err(ProviderError::Sse(message.to_string()));
            }
            // `ping` and unknown event types carry no payload we need.
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn finish(
        mut self,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<Response, ProviderError> {
        if let Some(text) = self.current_text.take() {
            self.blocks.push(ContentBlock::Text(text));
        }
        if let Some((id, name, buffer)) = self.current_tool.take() {
            let input = if buffer.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&buffer)?
            };
            self.blocks.push(ContentBlock::ToolUse { id, name, input });
            on_event(StreamEvent::ToolUseEnd);
        }
        Ok(Response {
            content: self.blocks,
            stop_reason: self.stop_reason.unwrap_or(StopReason::EndTurn),
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::provider::ToolSpec;

    fn sample_request() -> Request {
        Request {
            model: "claude-sonnet-4-5".into(),
            max_tokens: 1024,
            system: Some("You are fuji.".into()),
            messages: vec![
                Message::user("snapshot please"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "toolu_1".into(),
                        name: "desktop_snapshot".into(),
                        input: json!({}),
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "toolu_1".into(),
                        text: "{\"windows\":[]}".into(),
                        image: Some(ImageData {
                            media_type: "image/png".into(),
                            data: "aGVsbG8=".into(),
                        }),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "desktop_snapshot".into(),
                description: "Read the desktop.".into(),
                input_schema: json!({"type": "object"}),
            }],
        }
    }

    #[test]
    fn request_body_matches_messages_api_shape() {
        let body = request_body(&sample_request());
        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["system"], "You are fuji.");
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["name"], "desktop_snapshot");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["type"], "text");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["name"], "desktop_snapshot");
        let result = &messages[2]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["tool_use_id"], "toolu_1");
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][1]["type"], "image");
        assert_eq!(
            result["content"][1]["source"],
            json!({"type": "base64", "media_type": "image/png", "data": "aGVsbG8="})
        );
    }

    fn sse(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: Some(event.to_string()),
            data: data.to_string(),
        }
    }

    #[test]
    fn assembler_collects_text_and_tool_use_across_deltas() {
        let events = vec![
            sse(
                "message_start",
                r#"{"type":"message_start","message":{"usage":{"input_tokens":42}}}"#,
            ),
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello, "}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"world"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_9","name":"bash","input":{}}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":1}"#,
            ),
            sse(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":17}}"#,
            ),
            sse("message_stop", r#"{"type":"message_stop"}"#),
        ];
        let mut assembler = AnthropicAssembler::default();
        let mut seen = Vec::new();
        for event in &events {
            assembler
                .process(event, &mut |stream_event| seen.push(stream_event))
                .expect("process");
        }
        let response = assembler
            .finish(&mut |stream_event| seen.push(stream_event))
            .expect("finish");

        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.usage.input_tokens, 42);
        assert_eq!(response.usage.output_tokens, 17);
        assert_eq!(
            response.content,
            vec![
                ContentBlock::Text("Hello, world".into()),
                ContentBlock::ToolUse {
                    id: "toolu_9".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
            ]
        );
        assert!(seen.contains(&StreamEvent::ToolUseStart {
            id: "toolu_9".into(),
            name: "bash".into(),
        }));
        assert!(seen.contains(&StreamEvent::Done));
    }

    #[test]
    fn assembler_maps_stream_errors() {
        let mut assembler = AnthropicAssembler::default();
        let error = assembler
            .process(
                &sse(
                    "error",
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                ),
                &mut |_| {},
            )
            .expect_err("stream error");
        assert!(matches!(error, ProviderError::Sse(_)));
    }
}
