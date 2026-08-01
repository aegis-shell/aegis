//! OpenAI-compatible Chat Completions (`POST /chat/completions`) with SSE
//! streaming. Also covers DeepSeek, Qwen, and local endpoints that speak
//! the same shape.

use std::collections::BTreeMap;

use futures::StreamExt;
use serde_json::{Value, json};

use super::sse::{SseEvent, SseParser};
use super::{
    ContentBlock, Message, Provider, ProviderError, Request, Response, Role, StopReason,
    StreamEvent, Usage,
};

pub struct OpenAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
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

impl Provider for OpenAiProvider {
    async fn stream(
        &self,
        request: &Request,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<Response, ProviderError> {
        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
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
        let mut assembler = OpenAiAssembler::default();
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

/// Serialize the shared request into the Chat Completions wire shape.
fn request_body(request: &Request) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({"role": "system", "content": system}));
    }
    for message in &request.messages {
        wire_message(message, &mut messages);
    }
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "max_completion_tokens": request.max_tokens,
    });
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    },
                })
            })
            .collect();
    }
    body
}

fn wire_message(message: &Message, out: &mut Vec<Value>) {
    match message.role {
        Role::User => {
            let mut parts = Vec::new();
            for block in &message.content {
                match block {
                    ContentBlock::Text(text) => {
                        parts.push(json!({"type": "text", "text": text}));
                    }
                    ContentBlock::Image(image) => parts.push(image_part(image)),
                    ContentBlock::ToolResult {
                        tool_use_id,
                        text,
                        image,
                        ..
                    } => {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": text,
                        }));
                        if let Some(image) = image {
                            out.push(json!({
                                "role": "user",
                                "content": [image_part(image)],
                            }));
                        }
                    }
                    // A user message never carries tool calls.
                    ContentBlock::ToolUse { .. } => {}
                }
            }
            if !parts.is_empty() {
                out.push(json!({"role": "user", "content": parts}));
            }
        }
        Role::Assistant => {
            let text: String = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            let tool_calls = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => Some(json!({
                        "id": id,
                        "type": "function",
                        "function": {"name": name, "arguments": input.to_string()},
                    })),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let mut wire = json!({"role": "assistant"});
            wire["content"] = if text.is_empty() {
                Value::Null
            } else {
                json!(text)
            };
            if !tool_calls.is_empty() {
                wire["tool_calls"] = json!(tool_calls);
            }
            out.push(wire);
        }
    }
}

fn image_part(image: &super::ImageData) -> Value {
    json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{};base64,{}", image.media_type, image.data)},
    })
}

#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
    started: bool,
}

/// Folds Chat Completions chunks into stream callbacks and the final
/// [`Response`]. Kept HTTP-free so recorded streams can drive it in tests.
#[derive(Default)]
pub(crate) struct OpenAiAssembler {
    text: String,
    tools: BTreeMap<usize, ToolCallAcc>,
    stop_reason: Option<StopReason>,
    usage: Usage,
}

impl OpenAiAssembler {
    pub(crate) fn process(
        &mut self,
        event: &SseEvent,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<(), ProviderError> {
        let data = event.data.trim();
        if data == "[DONE]" {
            on_event(StreamEvent::Done);
            return Ok(());
        }
        let value: Value = serde_json::from_str(data)?;
        if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
            self.usage.input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
            self.usage.output_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);
        }
        let Some(choice) = value["choices"]
            .as_array()
            .and_then(|choices| choices.first())
        else {
            return Ok(());
        };
        if let Some(content) = choice["delta"]["content"].as_str() {
            self.text.push_str(content);
            on_event(StreamEvent::TextDelta(content.to_string()));
        }
        if let Some(tool_calls) = choice["delta"]["tool_calls"].as_array() {
            for call in tool_calls {
                let index = call["index"].as_u64().unwrap_or(0) as usize;
                let acc = self.tools.entry(index).or_default();
                if let Some(id) = call["id"].as_str() {
                    acc.id = id.to_string();
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    acc.name = name.to_string();
                }
                if !acc.started && !acc.id.is_empty() && !acc.name.is_empty() {
                    acc.started = true;
                    on_event(StreamEvent::ToolUseStart {
                        id: acc.id.clone(),
                        name: acc.name.clone(),
                    });
                }
                if let Some(fragment) = call["function"]["arguments"].as_str() {
                    acc.arguments.push_str(fragment);
                    on_event(StreamEvent::ToolUseInputDelta(fragment.to_string()));
                }
            }
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.stop_reason = Some(super::stop_reason(Some(reason)));
        }
        Ok(())
    }

    pub(crate) fn finish(
        mut self,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<Response, ProviderError> {
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentBlock::Text(std::mem::take(&mut self.text)));
        }
        for (_, acc) in std::mem::take(&mut self.tools) {
            let input = if acc.arguments.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&acc.arguments)?
            };
            content.push(ContentBlock::ToolUse {
                id: acc.id,
                name: acc.name,
                input,
            });
            on_event(StreamEvent::ToolUseEnd);
        }
        Ok(Response {
            content,
            stop_reason: self.stop_reason.unwrap_or(StopReason::EndTurn),
            usage: self.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::provider::{ImageData, ToolSpec};

    fn sample_request() -> Request {
        Request {
            model: "deepseek-chat".into(),
            max_tokens: 2048,
            system: Some("You are fuji.".into()),
            messages: vec![
                Message::user("look at this"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "read_image".into(),
                        input: json!({"path": "/tmp/a.png"}),
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        text: "capture".into(),
                        image: Some(ImageData {
                            media_type: "image/png".into(),
                            data: "aGVsbG8=".into(),
                        }),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "read_image".into(),
                description: "Read an image.".into(),
                input_schema: json!({"type": "object"}),
            }],
        }
    }

    #[test]
    fn request_body_matches_chat_completions_shape() {
        let body = request_body(&sample_request());
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read_image");

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(
            messages[0],
            json!({"role": "system", "content": "You are fuji."})
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"][0]["type"], "text");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(
            messages[2]["tool_calls"][0]["function"]["arguments"],
            json!({"path": "/tmp/a.png"}).to_string()
        );
        assert_eq!(
            messages[3],
            json!({"role": "tool", "tool_call_id": "call_1", "content": "capture"})
        );
        assert_eq!(
            messages[4],
            json!({
                "role": "user",
                "content": [{
                    "type": "image_url",
                    "image_url": {"url": "data:image/png;base64,aGVsbG8="},
                }],
            })
        );
    }

    fn chunk(data: &str) -> SseEvent {
        SseEvent {
            event: None,
            data: data.to_string(),
        }
    }

    #[test]
    fn assembler_collects_parallel_tool_calls_by_index() {
        let events = vec![
            chunk(
                r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"Checking."}}]}"#,
            ),
            chunk(
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"glob","arguments":"{\"pat"}}]}}]}"#,
            ),
            chunk(
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"grep","arguments":"{\"needle\":"}}]}}]}"#,
            ),
            chunk(
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"tern\":\"*.rs\"}"}}]}}]}"#,
            ),
            chunk(
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"\"fn main\"}"}}]}}]}"#,
            ),
            chunk(
                r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":7}}"#,
            ),
            chunk("[DONE]"),
        ];
        let mut assembler = OpenAiAssembler::default();
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
        assert_eq!(response.usage.input_tokens, 11);
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(
            response.content,
            vec![
                ContentBlock::Text("Checking.".into()),
                ContentBlock::ToolUse {
                    id: "call_a".into(),
                    name: "glob".into(),
                    input: json!({"pattern": "*.rs"}),
                },
                ContentBlock::ToolUse {
                    id: "call_b".into(),
                    name: "grep".into(),
                    input: json!({"needle": "fn main"}),
                },
            ]
        );
        assert!(seen.contains(&StreamEvent::Done));
        assert_eq!(
            seen.iter()
                .filter(|event| matches!(event, StreamEvent::ToolUseEnd))
                .count(),
            2
        );
    }
}
