use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

use base64::Engine;
use serde_json::{Value, json};

use crate::BridgeConfig;
use crate::tools::{AegisPlatform, PlatformError, ToolCallResult, ToolDefinition};

const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const SERVER_INSTRUCTIONS: &str = "Use desktop_snapshot before desktop ids. The Agent Interaction Domain handle is bridge-managed: never ask for or invent a raw Interaction Domain id. Capture before Interaction Domain input and verify queued effects with a fresh capture or journal.";

trait ToolHost {
    fn definitions(&mut self) -> Result<Vec<ToolDefinition>, PlatformError>;
    fn call(&mut self, name: &str, arguments: Value) -> Result<ToolCallResult, PlatformError>;
    fn shutdown(&mut self) -> Result<(), PlatformError>;
}

impl ToolHost for AegisPlatform {
    fn definitions(&mut self) -> Result<Vec<ToolDefinition>, PlatformError> {
        self.refreshed_definitions()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        self.call(name, arguments)
    }

    fn shutdown(&mut self) -> Result<(), PlatformError> {
        self.shutdown()
    }
}

struct LazyAegisPlatform {
    config: BridgeConfig,
    platform: Option<AegisPlatform>,
}

impl LazyAegisPlatform {
    fn platform(&mut self) -> Result<&mut AegisPlatform, PlatformError> {
        if self.platform.is_none() {
            self.platform = Some(AegisPlatform::connect(self.config.clone())?);
        }
        Ok(self.platform.as_mut().expect("platform created above"))
    }
}

impl ToolHost for LazyAegisPlatform {
    fn definitions(&mut self) -> Result<Vec<ToolDefinition>, PlatformError> {
        self.platform()?.refreshed_definitions()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        self.platform()?.call(name, arguments)
    }

    fn shutdown(&mut self) -> Result<(), PlatformError> {
        match self.platform.as_mut() {
            Some(platform) => platform.shutdown(),
            None => Ok(()),
        }
    }
}

/// Serve newline-delimited MCP JSON-RPC on stdin/stdout until the client
/// closes the pipe, then apply the configured graceful Interaction Domain shutdown policy.
pub fn serve(platform: &mut AegisPlatform) -> Result<(), McpError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    serve_io(platform, &mut reader, &mut writer)
}

/// Serve from configuration and connect to the native capability broker only
/// when the client first lists or calls a tool. This lets a
/// `server/discover` request remain side-effect free and fast: protocol
/// discovery must not create a principal, prompt the user, or acquire Interaction Domain
/// authority.
pub fn serve_config(config: BridgeConfig) -> Result<(), McpError> {
    let mut host = LazyAegisPlatform {
        config,
        platform: None,
    };
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    serve_io(&mut host, &mut reader, &mut writer)
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
        let line = match read_request_line(reader) {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(McpError::RequestTooLarge(limit)) => {
                write_json(
                    writer,
                    &rpc_error(
                        Value::Null,
                        -32600,
                        format!("request exceeds the {limit}-byte limit"),
                    ),
                )?;
                continue;
            }
            Err(error) => return Err(error),
        };
        let request: Value = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(error) => {
                write_json(writer, &rpc_error(Value::Null, -32700, error.to_string()))?;
                continue;
            }
        };
        if request.get("jsonrpc") != Some(&Value::String("2.0".into())) || !request.is_object() {
            write_json(
                writer,
                &rpc_error(Value::Null, -32600, "invalid JSON-RPC request"),
            )?;
            continue;
        }
        let id = request.get("id").cloned();
        if id
            .as_ref()
            .is_some_and(|id| !id.is_string() && !id.is_number() && !id.is_null())
        {
            write_json(
                writer,
                &rpc_error(Value::Null, -32600, "invalid request id"),
            )?;
            continue;
        }
        let method = request.get("method").and_then(Value::as_str);
        let Some(method) = method else {
            if let Some(id) = id {
                write_json(writer, &rpc_error(id, -32600, "request has no method"))?;
            }
            continue;
        };

        if id.is_none() {
            // Cancellation is accepted as a standard notification. Tool
            // calls execute serially, so it is only observable between calls;
            // dropping stdin remains the hard cancellation boundary.
            continue;
        }
        let id = id.expect("checked above");
        let response = handle_request(host, id, method, &request);
        write_json(writer, &response)?;
    }
}

fn handle_request(host: &mut dyn ToolHost, id: Value, method: &str, request: &Value) -> Value {
    if let Err(error) = validate_request(request) {
        return rpc_error_data(id, error.code, error.message, error.data);
    }
    match method {
        "server/discover" => complete_result(
            id,
            json!({
                "supportedVersions": [
                    MCP_PROTOCOL_VERSION
                ],
                "capabilities": {"tools": {"listChanged": false}},
                "instructions": SERVER_INSTRUCTIONS,
                "ttlMs": 0,
                "cacheScope": "private"
            }),
        ),
        "tools/list" => match host.definitions() {
            Ok(definitions) => complete_result(
                id,
                json!({
                    "tools": definitions
                        .iter()
                        .map(ToolDefinition::to_mcp)
                        .collect::<Vec<_>>(),
                    // The broker ceiling can change at any time. A host may
                    // retain the result but must treat it as immediately stale.
                    "ttlMs": 0,
                    "cacheScope": "private"
                }),
            ),
            Err(error) => rpc_error(id, -32603, error.to_string()),
        },
        "tools/call" => match parse_tool_call(request.get("params")) {
            Ok((name, arguments)) => {
                let result = match host.call(name, arguments) {
                    Ok(result) => render_tool_success(result),
                    Err(error) => render_tool_error(&error),
                };
                complete_result(id, result)
            }
            Err(message) => rpc_error(id, -32602, message),
        },
        _ => rpc_error(id, -32601, format!("method {method:?} is not supported")),
    }
}

struct RequestError {
    code: i32,
    message: &'static str,
    data: Option<Value>,
}

fn validate_request(request: &Value) -> Result<(), RequestError> {
    let Some(meta) = request.pointer("/params/_meta").and_then(Value::as_object) else {
        return Err(RequestError {
            code: -32602,
            message: "MCP 2026 requests require params._meta",
            data: None,
        });
    };
    let Some(version) = meta
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str)
    else {
        return Err(RequestError {
            code: -32602,
            message: "MCP 2026 requests require a protocol version",
            data: None,
        });
    };
    if version != MCP_PROTOCOL_VERSION {
        return Err(RequestError {
            code: -32022,
            message: "Unsupported protocol version",
            data: Some(json!({
                "requested": version,
                "supported": [MCP_PROTOCOL_VERSION]
            })),
        });
    }
    if !meta
        .get("io.modelcontextprotocol/clientCapabilities")
        .is_some_and(Value::is_object)
    {
        return Err(RequestError {
            code: -32602,
            message: "MCP 2026 requests require client capabilities",
            data: None,
        });
    }
    if let Some(info) = meta.get("io.modelcontextprotocol/clientInfo")
        && !valid_implementation(Some(info))
    {
        return Err(RequestError {
            code: -32602,
            message: "clientInfo must contain string name and version fields",
            data: None,
        });
    }
    Ok(())
}

fn valid_implementation(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        value.get("name").is_some_and(Value::is_string)
            && value.get("version").is_some_and(Value::is_string)
    })
}

fn server_info() -> Value {
    json!({"name": "aegis-mcp", "version": env!("CARGO_PKG_VERSION")})
}

fn complete_result(id: Value, mut result: Value) -> Value {
    if let Some(result) = result.as_object_mut() {
        result.insert("resultType".into(), Value::String("complete".into()));
        result.insert(
            "_meta".into(),
            json!({"io.modelcontextprotocol/serverInfo": server_info()}),
        );
    }
    rpc_result(id, result)
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
    rpc_error_data(id, code, message, None)
}

fn rpc_error_data(id: Value, code: i32, message: impl Into<String>, data: Option<Value>) -> Value {
    let mut error = json!({"code": code, "message": message.into()});
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error
    })
}

fn write_json(writer: &mut dyn Write, value: &Value) -> Result<(), McpError> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_request_line(reader: &mut dyn BufRead) -> Result<Option<Vec<u8>>, McpError> {
    loop {
        let mut line = Vec::new();
        let bytes = {
            let mut limited = reader.take((MAX_REQUEST_BYTES + 1) as u64);
            limited.read_until(b'\n', &mut line)?
        };
        if bytes == 0 {
            return Ok(None);
        }
        if line.len() > MAX_REQUEST_BYTES {
            // Drain without allocating in proportion to attacker input.
            if !line.ends_with(b"\n") {
                loop {
                    let available = reader.fill_buf()?;
                    if available.is_empty() {
                        break;
                    }
                    let consumed = available
                        .iter()
                        .position(|byte| *byte == b'\n')
                        .map_or(available.len(), |position| position + 1);
                    let done = consumed <= available.len() && available[consumed - 1] == b'\n';
                    reader.consume(consumed);
                    if done {
                        break;
                    }
                }
            }
            return Err(McpError::RequestTooLarge(MAX_REQUEST_BYTES));
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if !line.is_empty() {
            return Ok(Some(line));
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP stdio failed: {0}")]
    Io(#[from] io::Error),
    #[error("MCP response encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("MCP request exceeds the {0}-byte limit")]
    RequestTooLarge(usize),
    #[error("graceful Interaction Domain shutdown failed; recovery state was retained: {0}")]
    Shutdown(PlatformError),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeHost {
        shutdown: bool,
    }

    impl ToolHost for FakeHost {
        fn definitions(&mut self) -> Result<Vec<ToolDefinition>, PlatformError> {
            Ok(vec![ToolDefinition {
                name: "status",
                description: "status",
                input_schema: json!({"type": "object"}),
                read_only: true,
                destructive: false,
            }])
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
    fn stdio_discovers_lists_and_calls() {
        let meta = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}"#;
        let input = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"server/discover\",\"params\":{{{meta}}}}}\n\
             {{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{{{meta}}}}}\n\
             {{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{{\"name\":\"status\",\"arguments\":{{}},{meta}}}}}\n"
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
        assert_eq!(responses[0]["result"]["resultType"], "complete");
        assert_eq!(
            responses[0]["result"]["supportedVersions"][0],
            MCP_PROTOCOL_VERSION
        );
        assert_eq!(responses[1]["result"]["cacheScope"], "private");
        assert_eq!(responses[1]["result"]["ttlMs"], 0);
        assert_eq!(responses[2]["result"]["isError"], false);
        for response in responses {
            assert_eq!(
                response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
                "aegis-mcp"
            );
        }
    }

    #[test]
    fn requests_fail_closed_on_missing_or_unknown_metadata() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"server/discover\",\"params\":{\"_meta\":{\"io.modelcontextprotocol/protocolVersion\":\"2026-07-28\",\"io.modelcontextprotocol/clientCapabilities\":{}}}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{\"_meta\":{\"io.modelcontextprotocol/protocolVersion\":\"2099-01-01\",\"io.modelcontextprotocol/clientCapabilities\":{}}}}\n"
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
        assert_eq!(responses[1]["error"]["code"], -32602);
        assert_eq!(responses[2]["error"]["code"], -32022);
        assert_eq!(responses[2]["error"]["data"]["requested"], "2099-01-01");
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

    #[test]
    fn oversized_frame_is_drained_and_the_next_request_survives() {
        let mut input = vec![b'x'; MAX_REQUEST_BYTES + 1];
        input.extend_from_slice(
            b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"server/discover\",\"params\":{\"_meta\":{\"io.modelcontextprotocol/protocolVersion\":\"2026-07-28\",\"io.modelcontextprotocol/clientCapabilities\":{}}}}\n",
        );
        let mut reader = BufReader::new(input.as_slice());
        let mut output = Vec::new();
        let mut host = FakeHost { shutdown: false };
        serve_io(&mut host, &mut reader, &mut output).expect("serve");
        let responses = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).expect("json"))
            .collect::<Vec<_>>();
        assert_eq!(responses[0]["error"]["code"], -32600);
        assert_eq!(
            responses[1]["result"]["supportedVersions"][0],
            MCP_PROTOCOL_VERSION
        );
    }
}
