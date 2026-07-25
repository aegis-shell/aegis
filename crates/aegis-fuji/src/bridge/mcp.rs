use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

use base64::Engine;
use serde_json::{Value, json};

use crate::bridge::tools::{AssPlatform, PlatformError, ToolCallResult, ToolDefinition};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

trait ToolHost {
    fn definitions(&self) -> Vec<ToolDefinition>;
    fn call(&mut self, name: &str, arguments: Value) -> Result<ToolCallResult, PlatformError>;
    fn shutdown(&mut self) -> Result<(), PlatformError>;
}

impl ToolHost for AssPlatform {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        self.call(name, arguments)
    }

    fn shutdown(&mut self) -> Result<(), PlatformError> {
        self.shutdown()
    }
}

/// Serve newline-delimited MCP JSON-RPC on stdin/stdout until the client
/// closes the pipe, then apply the configured graceful Realm shutdown policy.
pub fn serve(platform: &mut AssPlatform) -> Result<(), McpError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    serve_io(platform, &mut reader, &mut writer)
}

fn serve_io(
    host: &mut dyn ToolHost,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> Result<(), McpError> {
    let loop_result = serve_loop(host, reader, writer);
    let shutdown_result = host.shutdown();
    match (loop_result, shutdown_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(McpError::Shutdown(error)),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn serve_loop(
    host: &mut dyn ToolHost,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> Result<(), McpError> {
    loop {
        let Some(line) = read_request_line(reader)? else {
            return Ok(());
        };
        let request: Value = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(error) => {
                write_json(writer, &rpc_error(Value::Null, -32700, error.to_string()))?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str);
        let Some(method) = method else {
            if let Some(id) = id {
                write_json(writer, &rpc_error(id, -32600, "request has no method"))?;
            }
            continue;
        };

        if id.is_none() {
            // MCP notifications are intentionally side-effect free here. The
            // initialized/cancelled notifications need no server response.
            continue;
        }
        let id = id.expect("checked above");
        let response = match method {
            "initialize" => rpc_result(
                id,
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {
                        "name": "aegis-fuji",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": "Use desktop_snapshot before desktop ids. fuji's Realm id is bridge-managed: never ask for or invent one. Capture before Realm input and verify queued effects with a fresh capture or journal."
                }),
            ),
            "ping" => rpc_result(id, json!({})),
            "tools/list" => rpc_result(
                id,
                json!({
                    "tools": host
                        .definitions()
                        .iter()
                        .map(ToolDefinition::to_mcp)
                        .collect::<Vec<_>>()
                }),
            ),
            "tools/call" => match parse_tool_call(request.get("params")) {
                Ok((name, arguments)) => {
                    let result = match host.call(name, arguments) {
                        Ok(result) => render_tool_success(result),
                        Err(error) => render_tool_error(&error),
                    };
                    rpc_result(id, result)
                }
                Err(message) => rpc_error(id, -32602, message),
            },
            "shutdown" => {
                write_json(writer, &rpc_result(id, Value::Null))?;
                return Ok(());
            }
            _ => rpc_error(id, -32601, format!("method {method:?} is not supported")),
        };
        write_json(writer, &response)?;
    }
}

fn parse_tool_call(params: Option<&Value>) -> Result<(&str, Value), String> {
    let params = params.ok_or_else(|| "tools/call params are required".to_string())?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call params.name must be a string".to_string())?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() && !arguments.is_null() {
        return Err("tools/call params.arguments must be an object".into());
    }
    Ok((name, arguments))
}

fn render_tool_success(result: ToolCallResult) -> Value {
    let text = serde_json::to_string(&result.value)
        .unwrap_or_else(|error| format!("{{\"serialization_error\":{error:?}}}"));
    let mut content = vec![json!({"type": "text", "text": text})];
    if let Some(png) = result.image_png {
        content.push(json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(png),
            "mimeType": "image/png"
        }));
    }
    json!({
        "content": content,
        "structuredContent": result.value,
        "isError": false
    })
}

fn render_tool_error(error: &PlatformError) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": json!({
                "status": "error",
                "message": error.to_string()
            }).to_string()
        }],
        "isError": true
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()}
    })
}

fn write_json(writer: &mut dyn Write, value: &Value) -> Result<(), McpError> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_request_line(reader: &mut dyn BufRead) -> Result<Option<Vec<u8>>, McpError> {
    let mut line = Vec::new();
    let bytes = {
        let mut limited = reader.take((MAX_REQUEST_BYTES + 1) as u64);
        limited.read_until(b'\n', &mut line)?
    };
    if bytes == 0 {
        return Ok(None);
    }
    if line.len() > MAX_REQUEST_BYTES {
        // Drain the rest of this frame so a following valid request is still
        // parseable after the bounded rejection.
        if !line.ends_with(b"\n") {
            let mut discard = Vec::new();
            reader.read_until(b'\n', &mut discard)?;
        }
        return Err(McpError::RequestTooLarge(MAX_REQUEST_BYTES));
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    if line.is_empty() {
        return read_request_line(reader);
    }
    Ok(Some(line))
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP stdio failed: {0}")]
    Io(#[from] io::Error),
    #[error("MCP response encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("MCP request exceeds the {0}-byte limit")]
    RequestTooLarge(usize),
    #[error("graceful Realm shutdown failed; recovery state was retained: {0}")]
    Shutdown(PlatformError),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeHost {
        shutdown: bool,
    }

    impl ToolHost for FakeHost {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "status",
                description: "status",
                input_schema: json!({"type": "object"}),
                read_only: true,
                destructive: false,
            }]
        }

        fn call(&mut self, name: &str, _arguments: Value) -> Result<ToolCallResult, PlatformError> {
            if name != "status" {
                return Err(PlatformError::UnknownTool(name.into()));
            }
            Ok(ToolCallResult::json(json!({"ok": true})))
        }

        fn shutdown(&mut self) -> Result<(), PlatformError> {
            self.shutdown = true;
            Ok(())
        }
    }

    #[test]
    fn initializes_lists_and_calls_over_newline_json_rpc() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"status\",\"arguments\":{}}}\n"
        );
        let mut reader = BufReader::new(input.as_bytes());
        let mut output = Vec::new();
        let mut host = FakeHost { shutdown: false };

        serve_io(&mut host, &mut reader, &mut output).expect("serve");

        let responses = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).expect("json"))
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 3);
        assert_eq!(
            responses[0]["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
        assert_eq!(responses[1]["result"]["tools"][0]["name"], "status");
        assert_eq!(responses[2]["result"]["isError"], false);
        assert!(host.shutdown);
    }

    #[test]
    fn malformed_json_gets_parse_error_without_ending_session() {
        let mut reader = BufReader::new(b"{bad}\n".as_slice());
        let mut output = Vec::new();
        let mut host = FakeHost { shutdown: false };
        serve_io(&mut host, &mut reader, &mut output).expect("serve");
        let response: Value =
            serde_json::from_slice(output.strip_suffix(b"\n").expect("newline")).expect("response");
        assert_eq!(response["error"]["code"], -32700);
    }
}
