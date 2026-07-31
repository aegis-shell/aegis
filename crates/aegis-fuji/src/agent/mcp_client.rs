//! Stdio MCP client: newline-delimited JSON-RPC against servers such as
//! `aegis-mcp`. The transport is generic over async reader/writer pairs
//! so tests can drive it through an in-memory duplex.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::agent::config::McpServerConfig;
use crate::agent::provider::{ImageData, ToolSpec};
use crate::agent::tools::ToolOutput;

const PROTOCOL_VERSION: &str = "2024-11-05";
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// One tool advertised by the server, under its connector-local name.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

/// A live stdio MCP server process with its tool catalog cached.
pub struct McpClient {
    server: String,
    conn: JsonRpcConn<BufReader<ChildStdout>, ChildStdin>,
    child: Child,
    tools: Vec<McpTool>,
}

impl McpClient {
    /// Spawn the server, run `initialize` + `tools/list`, and cache tools.
    pub async fn spawn(server: &str, config: &McpServerConfig) -> Result<Self, McpError> {
        let (program, args) = config.command.split_first().ok_or(McpError::EmptyCommand)?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .envs(&config.environment);
        let mut child = command.spawn().map_err(|source| McpError::Spawn {
            program: program.clone(),
            source,
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Protocol("child stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Protocol("child stdout unavailable".into()))?;
        let mut conn = JsonRpcConn::new(BufReader::new(stdout), stdin);
        let tools = conn.handshake().await?;
        Ok(Self {
            server: server.to_string(),
            conn,
            child,
            tools,
        })
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Model-facing specs namespaced as `mcp__<server>__<tool>`.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|tool| ToolSpec {
                name: format!("mcp__{}__{}", self.server, tool.name),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect()
    }

    /// Read-only flag for one connector-local tool name.
    pub fn is_read_only(&self, name: &str) -> bool {
        self.tools
            .iter()
            .any(|tool| tool.name == name && tool.read_only)
    }

    /// Call one connector-local tool.
    pub async fn call(&mut self, name: &str, arguments: Value) -> Result<ToolOutput, McpError> {
        let result = tokio::time::timeout(
            CALL_TIMEOUT,
            self.conn
                .request("tools/call", json!({"name": name, "arguments": arguments})),
        )
        .await
        .map_err(|_| McpError::Timeout(CALL_TIMEOUT))??;
        Ok(parse_tool_result(&result))
    }

    /// Best-effort graceful shutdown: request, close stdin, wait, then kill.
    pub async fn shutdown(&mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let _ = self.conn.request("shutdown", json!({})).await;
            let _ = self.child.wait().await;
        })
        .await;
        let _ = self.child.kill().await;
    }
}

/// Newline-delimited JSON-RPC over any async reader/writer pair.
pub(crate) struct JsonRpcConn<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
}

impl<R, W> JsonRpcConn<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub(crate) fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: 0,
        }
    }

    /// `initialize` + `notifications/initialized` + `tools/list`.
    pub(crate) async fn handshake(&mut self) -> Result<Vec<McpTool>, McpError> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            self.request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "fuji", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
            self.notify("notifications/initialized").await?;
            let result = self.request("tools/list", json!({})).await?;
            parse_tools(&result)
        })
        .await
        .map_err(|_| McpError::Timeout(HANDSHAKE_TIMEOUT))?
    }

    pub(crate) async fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        self.next_id += 1;
        let id = self.next_id;
        self.write_json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        loop {
            let line = self.read_line().await?;
            let message: Value = serde_json::from_str(&line)?;
            match message.get("id").and_then(Value::as_u64) {
                Some(seen) if seen == id => {
                    if let Some(error) = message.get("error") {
                        return Err(McpError::Rpc {
                            code: error["code"].as_i64().unwrap_or(0),
                            message: error["message"]
                                .as_str()
                                .unwrap_or("unknown MCP error")
                                .to_string(),
                        });
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                // Notifications and stale responses carry no reply we await.
                _ => continue,
            }
        }
    }

    pub(crate) async fn notify(&mut self, method: &str) -> Result<(), McpError> {
        self.write_json(&json!({"jsonrpc": "2.0", "method": method}))
            .await
    }

    async fn write_json(&mut self, value: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<String, McpError> {
        loop {
            let mut line = String::new();
            let bytes = self.reader.read_line(&mut line).await?;
            if bytes == 0 {
                return Err(McpError::Eof);
            }
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
}

fn parse_tools(result: &Value) -> Result<Vec<McpTool>, McpError> {
    let tools = result["tools"]
        .as_array()
        .ok_or_else(|| McpError::Protocol("tools/list result has no tools array".into()))?;
    Ok(tools
        .iter()
        .filter_map(|tool| {
            Some(McpTool {
                name: tool["name"].as_str()?.to_string(),
                description: tool["description"].as_str().unwrap_or_default().to_string(),
                input_schema: tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"})),
                read_only: tool["annotations"]["readOnlyHint"]
                    .as_bool()
                    .unwrap_or(false),
            })
        })
        .collect())
}

fn parse_tool_result(result: &Value) -> ToolOutput {
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let mut text_parts = Vec::new();
    let mut image = None;
    if let Some(content) = result["content"].as_array() {
        for part in content {
            match part["type"].as_str() {
                Some("text") => {
                    if let Some(text) = part["text"].as_str() {
                        text_parts.push(text.to_string());
                    }
                }
                Some("image") if image.is_none() => {
                    image = Some(ImageData {
                        media_type: part["mimeType"].as_str().unwrap_or("image/png").to_string(),
                        data: part["data"].as_str().unwrap_or_default().to_string(),
                    });
                }
                _ => {}
            }
        }
    }
    if text_parts.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        text_parts.push(structured.to_string());
    }
    ToolOutput {
        text: text_parts.join("\n"),
        image,
        is_error,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP server command is empty")]
    EmptyCommand,
    #[error("cannot spawn MCP server {program:?}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("MCP I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("MCP JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("MCP server closed the connection")]
    Eof,
    #[error("MCP call timed out after {0:?}")]
    Timeout(Duration),
    #[error("MCP error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("MCP protocol violation: {0}")]
    Protocol(String),
}

#[cfg(test)]
mod tests {
    use tokio::io::{BufReader, DuplexStream, ReadHalf, WriteHalf, duplex};

    use super::*;

    type TestConn = JsonRpcConn<BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>>;

    /// A scripted server: answers initialize/tools/list/tools/call and one
    /// `failing_method` with an RPC error. Interleaves a notification before
    /// every response to exercise skipping.
    fn fake_server() -> (TestConn, tokio::task::JoinHandle<()>) {
        let (client_end, server_end) = duplex(64 * 1024);
        let (server_read, mut server_write) = tokio::io::split(server_end);
        let handle = tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
                let Ok(request) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(id) = request.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let method = request["method"].as_str().unwrap_or("");
                let response = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"protocolVersion": "2024-11-05", "capabilities": {},
                                   "serverInfo": {"name": "fake", "version": "0"}},
                    }),
                    "tools/list" => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"tools": [{
                            "name": "echo",
                            "description": "echo back",
                            "inputSchema": {"type": "object"},
                            "annotations": {"readOnlyHint": true},
                        }]},
                    }),
                    "tools/call" => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "content": [
                                {"type": "text", "text": "hi"},
                                {"type": "image", "data": "aGk=", "mimeType": "image/png"},
                            ],
                            "isError": false,
                        },
                    }),
                    _ => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "no such method"},
                    }),
                };
                let notification = json!({"jsonrpc": "2.0", "method": "notifications/progress"});
                let mut frame = serde_json::to_vec(&notification).expect("json");
                frame.push(b'\n');
                frame.extend_from_slice(&serde_json::to_vec(&response).expect("json"));
                frame.push(b'\n');
                if server_write.write_all(&frame).await.is_err() {
                    break;
                }
                let _ = server_write.flush().await;
            }
        });
        let (client_read, client_write) = tokio::io::split(client_end);
        (
            JsonRpcConn::new(BufReader::new(client_read), client_write),
            handle,
        )
    }

    #[tokio::test]
    async fn handshake_caches_tools_with_annotations() {
        let (mut conn, _server) = fake_server();
        let tools = conn.handshake().await.expect("handshake");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert!(tools[0].read_only);
        assert_eq!(tools[0].input_schema, json!({"type": "object"}));
    }

    #[tokio::test]
    async fn tool_call_parses_text_and_image_content() {
        let (mut conn, _server) = fake_server();
        let result = conn
            .request("tools/call", json!({"name": "echo", "arguments": {}}))
            .await
            .expect("call");
        let output = parse_tool_result(&result);
        assert!(!output.is_error);
        assert_eq!(output.text, "hi");
        let image = output.image.expect("image");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.data, "aGk=");
    }

    #[tokio::test]
    async fn rpc_errors_surface_code_and_message() {
        let (mut conn, _server) = fake_server();
        let error = conn
            .request("bogus", json!({}))
            .await
            .expect_err("rpc error");
        assert!(matches!(error, McpError::Rpc { code: -32601, .. }));
    }
}
