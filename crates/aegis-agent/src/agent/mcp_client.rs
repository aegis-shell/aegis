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

const PROTOCOL_VERSION: &str = "2026-07-28";
const CALL_TIMEOUT: Duration = Duration::from_secs(370);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(370);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 300 * 1024 * 1024;

type ChildConn = JsonRpcConn<BufReader<ChildStdout>, ChildStdin>;

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
    conn: ChildConn,
    child: Child,
    tools: Vec<McpTool>,
}

impl McpClient {
    /// Spawn a 2026-07-28 server, validate discovery, and cache its tool
    /// catalog.
    pub async fn spawn(server: &str, config: &McpServerConfig) -> Result<Self, McpError> {
        let (program, args) = config.command.split_first().ok_or(McpError::EmptyCommand)?;
        let (child, mut conn) = spawn_transport(program, args, config)?;
        tokio::time::timeout(DISCOVERY_TIMEOUT, conn.discover())
            .await
            .map_err(|_| McpError::Timeout(DISCOVERY_TIMEOUT))??;
        let tools = tokio::time::timeout(CATALOG_TIMEOUT, conn.list_tools())
            .await
            .map_err(|_| McpError::Timeout(CATALOG_TIMEOUT))??;
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
        let result = self
            .conn
            .request_with_timeout(
                "tools/call",
                json!({"name": name, "arguments": arguments}),
                CALL_TIMEOUT,
            )
            .await?;
        Ok(parse_tool_result(&result))
    }

    /// Best-effort standard stdio shutdown: close stdin, wait, then terminate.
    /// MCP does not define a `shutdown` JSON-RPC method.
    pub async fn shutdown(&mut self) {
        let _ = self.conn.close_writer().await;
        if tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
    }
}

fn spawn_transport(
    program: &str,
    args: &[String],
    config: &McpServerConfig,
) -> Result<(Child, ChildConn), McpError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .envs(&config.environment)
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|source| McpError::Spawn {
        program: program.to_string(),
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
    Ok((child, JsonRpcConn::new(BufReader::new(stdout), stdin)))
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

    /// Discover the stateless server and require the exact protocol version
    /// used for every subsequent request.
    async fn discover(&mut self) -> Result<(), McpError> {
        let discovered = self.request("server/discover", json!({})).await?;
        let supported = discovered["supportedVersions"].as_array().ok_or_else(|| {
            McpError::Protocol("server/discover result has no supportedVersions array".into())
        })?;
        if !supported
            .iter()
            .any(|version| version.as_str() == Some(PROTOCOL_VERSION))
        {
            return Err(McpError::Protocol(format!(
                "server/discover does not advertise {PROTOCOL_VERSION}"
            )));
        }
        Ok(())
    }

    async fn list_tools(&mut self) -> Result<Vec<McpTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        parse_tools(&result)
    }

    pub(crate) async fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.send_request(method, params).await?;
        self.wait_response(id).await
    }

    async fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let id = self.send_request(method, params).await?;
        match tokio::time::timeout(timeout, self.wait_response(id)).await {
            Ok(result) => result,
            Err(_) => {
                let _ = self
                    .notify_with_params("notifications/cancelled", json!({"requestId": id}))
                    .await;
                Err(McpError::Timeout(timeout))
            }
        }
    }

    async fn send_request(&mut self, method: &str, params: Value) -> Result<u64, McpError> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| McpError::Protocol("request id space exhausted".into()))?;
        let id = self.next_id;
        let params = self.decorate_params(params)?;
        self.write_json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        Ok(id)
    }

    fn decorate_params(&self, mut params: Value) -> Result<Value, McpError> {
        let params_object = params
            .as_object_mut()
            .ok_or_else(|| McpError::Protocol("MCP request params must be an object".into()))?;
        let mut meta_value = params_object.remove("_meta").unwrap_or_else(|| json!({}));
        {
            let meta = meta_value.as_object_mut().ok_or_else(|| {
                McpError::Protocol("MCP request params._meta must be an object".into())
            })?;
            meta.insert(
                "io.modelcontextprotocol/protocolVersion".into(),
                Value::String(PROTOCOL_VERSION.into()),
            );
            meta.insert(
                "io.modelcontextprotocol/clientInfo".into(),
                json!({"name": "aegis-agent", "version": env!("CARGO_PKG_VERSION")}),
            );
            meta.insert(
                "io.modelcontextprotocol/clientCapabilities".into(),
                json!({}),
            );
        }
        params_object.insert("_meta".into(), meta_value);
        Ok(params)
    }

    async fn wait_response(&mut self, id: u64) -> Result<Value, McpError> {
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
                            data: error.get("data").cloned(),
                        });
                    }
                    let result = message.get("result").cloned().unwrap_or(Value::Null);
                    if result.get("resultType").and_then(Value::as_str) != Some("complete") {
                        return Err(McpError::Protocol(
                            "MCP response is not a complete result".into(),
                        ));
                    }
                    return Ok(result);
                }
                // Notifications and stale responses carry no reply we await.
                _ => continue,
            }
        }
    }

    async fn notify_with_params(
        &mut self,
        method: &str,
        mut params: Value,
    ) -> Result<(), McpError> {
        params = self.decorate_params(params)?;
        self.write_json(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn close_writer(&mut self) -> Result<(), McpError> {
        self.writer.shutdown().await.map_err(Into::into)
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
            let mut line = Vec::new();
            loop {
                let available = self.reader.fill_buf().await?;
                if available.is_empty() {
                    if line.is_empty() {
                        return Err(McpError::Eof);
                    }
                    break;
                }
                let newline = available.iter().position(|byte| *byte == b'\n');
                let consumed = newline.map_or(available.len(), |position| position + 1);
                let remaining = MAX_RESPONSE_BYTES
                    .saturating_add(1)
                    .saturating_sub(line.len());
                line.extend_from_slice(&available[..consumed.min(remaining)]);
                self.reader.consume(consumed);
                if line.len() > MAX_RESPONSE_BYTES {
                    if newline.is_none() {
                        loop {
                            let available = self.reader.fill_buf().await?;
                            if available.is_empty() {
                                break;
                            }
                            let newline = available.iter().position(|byte| *byte == b'\n');
                            let consumed = newline.map_or(available.len(), |position| position + 1);
                            self.reader.consume(consumed);
                            if newline.is_some() {
                                break;
                            }
                        }
                    }
                    return Err(McpError::ResponseTooLarge(MAX_RESPONSE_BYTES));
                }
                if newline.is_some() {
                    break;
                }
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            if !line.is_empty() {
                return String::from_utf8(line)
                    .map_err(|_| McpError::Protocol("MCP response is not UTF-8".into()));
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
    #[error("MCP response exceeds the {0}-byte limit")]
    ResponseTooLarge(usize),
    #[error("MCP call timed out after {0:?}")]
    Timeout(Duration),
    #[error("MCP error {code}: {message}")]
    Rpc {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("MCP protocol violation: {0}")]
    Protocol(String),
}

#[cfg(test)]
mod tests {
    use tokio::io::{BufReader, DuplexStream, ReadHalf, WriteHalf, duplex};

    use super::*;

    type TestConn = JsonRpcConn<BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>>;

    /// A scripted server. It interleaves a notification before every
    /// response to exercise response correlation.
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
                    "server/discover" => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "resultType": "complete",
                            "supportedVersions": [PROTOCOL_VERSION],
                            "capabilities": {"tools": {"listChanged": false}},
                            "ttlMs": 0,
                            "cacheScope": "private",
                            "_meta": {"io.modelcontextprotocol/serverInfo": {
                                "name": "fake", "version": "0"
                            }}
                        },
                    }),
                    "tools/list" => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "resultType": "complete",
                            "tools": [{
                                "name": "echo",
                                "description": "echo back",
                                "inputSchema": {"type": "object"},
                                "annotations": {"readOnlyHint": true},
                            }],
                            "ttlMs": 0,
                            "cacheScope": "private"
                        },
                    }),
                    "tools/call" => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "resultType": "complete",
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
    async fn discovery_caches_tools_with_annotations() {
        let (mut conn, _server) = fake_server();
        conn.discover().await.expect("discovery");
        let tools = conn.list_tools().await.expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert!(tools[0].read_only);
        assert_eq!(tools[0].input_schema, json!({"type": "object"}));
    }

    #[tokio::test]
    async fn tool_call_parses_text_and_image_content() {
        let (mut conn, _server) = fake_server();
        conn.discover().await.expect("discovery");
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
        conn.discover().await.expect("discovery");
        let error = conn
            .request("bogus", json!({}))
            .await
            .expect_err("rpc error");
        assert!(matches!(error, McpError::Rpc { code: -32601, .. }));
    }
}
