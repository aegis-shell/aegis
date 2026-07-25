//! The agent loop: provider stream ↔ tool dispatch, bounded by permissions
//! and a turn limit.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::agent::config::{AgentConfig, FujiConfig};
use crate::agent::mcp_client::McpClient;
use crate::agent::permissions::{Decision, PermissionGate};
use crate::agent::provider::{
    AnyProvider, ContentBlock, Message, Provider, ProviderError, Request, Role, StopReason,
    StreamEvent, Usage,
};
use crate::agent::session::timestamp;
use crate::agent::skills::{self, Skill};
use crate::agent::tools::{ToolOutput, ToolRegistry, parse_mcp_name};

const SYSTEM_PROMPT: &str = "\
You are fuji (宓姬), the agent software of the ASS desktop. You run in a terminal and help with \
whatever the user asks: explanation, writing, and code work through your tools, plus operating \
the desktop itself when desktop tools are connected.

Rules that always apply:
- Use tools to act on the world; never claim you changed something you did not verify.
- A `status: queued` style result means intent accepted, not effect applied. Verify with a fresh \
observation (snapshot, journal, or capture) before reporting success.
- When `mcp__ass__*` desktop tools are present, read the `ass-desktop-realm` skill with \
skill_read before operating windows, applications, or Realms.
- Never use destructive operations (close_window, realm_reset, file deletion) without explicit \
user intent.";

/// Progress of one run, streamed to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    TextDelta(String),
    ToolCall { name: String },
}

/// Summary of one completed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub text: String,
    pub stop_reason: StopReason,
    pub usage: Usage,
    pub turns: u32,
}

/// The agent: provider, tools, MCP connections, permissions, and limits.
pub struct Agent<P: Provider> {
    provider: P,
    registry: ToolRegistry,
    mcp: BTreeMap<String, McpClient>,
    gate: PermissionGate,
    system: String,
    model: String,
    max_tokens: u32,
    max_turns: u32,
}

impl Agent<AnyProvider> {
    /// Assemble from loaded configuration. MCP servers that fail to start
    /// are reported on stderr and skipped, never fatal.
    pub async fn from_config(
        config: &FujiConfig,
        auto_approve: bool,
        model_override: Option<String>,
    ) -> Result<Self, AgentError> {
        let resolved = config.provider.resolve();
        let provider = AnyProvider::from_config(&resolved, reqwest::Client::new())?;
        let skills = skills::discover(&config.skills.paths);
        let mut registry = ToolRegistry::new(skills.clone());
        let mut mcp = BTreeMap::new();
        for (name, server) in &config.mcp {
            if !server.enabled {
                continue;
            }
            match McpClient::spawn(name, server).await {
                Ok(client) => {
                    let tools = client
                        .tool_specs()
                        .into_iter()
                        .map(|spec| {
                            let local = spec
                                .name
                                .strip_prefix(&format!("mcp__{name}__"))
                                .unwrap_or(&spec.name)
                                .to_string();
                            (spec, client.is_read_only(&local))
                        })
                        .collect();
                    registry.register_mcp_tools(tools);
                    mcp.insert(name.clone(), client);
                }
                Err(error) => {
                    eprintln!("fuji: MCP server {name:?} failed to start: {error}");
                }
            }
        }
        let has_ass = mcp.keys().any(|name| name == "ass");
        let system = system_prompt(&config.agent, &skills, has_ass);
        Ok(Self::assemble(
            provider,
            registry,
            mcp,
            PermissionGate::new(config.permissions.clone(), auto_approve),
            system,
            model_override.unwrap_or(resolved.model),
            resolved.max_tokens,
            config.agent.max_turns,
        ))
    }
}

impl<P: Provider> Agent<P> {
    /// Direct constructor, primarily for tests with a mock provider.
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        provider: P,
        registry: ToolRegistry,
        mcp: BTreeMap<String, McpClient>,
        gate: PermissionGate,
        system: String,
        model: String,
        max_tokens: u32,
        max_turns: u32,
    ) -> Self {
        Self {
            provider,
            registry,
            mcp,
            gate,
            system,
            model,
            max_tokens,
            max_turns,
        }
    }

    pub fn set_max_turns(&mut self, max_turns: u32) {
        self.max_turns = max_turns;
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect()
    }

    pub fn mcp_servers(&self) -> Vec<(&str, usize)> {
        self.mcp
            .values()
            .map(|client| (client.server(), client.tools().len()))
            .collect()
    }

    /// Gracefully stop every MCP server.
    pub async fn shutdown(&mut self) {
        for client in self.mcp.values_mut() {
            client.shutdown().await;
        }
    }

    /// Run the loop for one user prompt. Appends the user message, every
    /// assistant turn, and every tool result to `messages`.
    pub async fn run(
        &mut self,
        messages: &mut Vec<Message>,
        prompt: impl Into<String>,
        on_event: &mut dyn FnMut(AgentEvent),
    ) -> Result<TurnOutcome, AgentError> {
        messages.push(Message::user(prompt));
        let mut usage = Usage::default();
        let mut turns = 0;
        loop {
            turns += 1;
            if turns > self.max_turns {
                return Err(AgentError::MaxTurns(self.max_turns));
            }
            let request = Request {
                model: self.model.clone(),
                max_tokens: self.max_tokens,
                system: Some(self.system.clone()),
                messages: messages.clone(),
                tools: self.registry.specs(),
            };
            let response = self
                .provider
                .stream(&request, &mut |event| {
                    if let StreamEvent::TextDelta(text) = event {
                        on_event(AgentEvent::TextDelta(text));
                    }
                })
                .await?;
            usage.input_tokens = response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;

            let tool_calls = response
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let text: String = response
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            messages.push(Message {
                role: Role::Assistant,
                content: response.content,
            });

            if response.stop_reason != StopReason::ToolUse || tool_calls.is_empty() {
                return Ok(TurnOutcome {
                    text,
                    stop_reason: response.stop_reason,
                    usage,
                    turns,
                });
            }

            let mut results = Vec::new();
            for (id, name, input) in tool_calls {
                on_event(AgentEvent::ToolCall { name: name.clone() });
                let output = self.execute(&name, input).await;
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id,
                    text: output.text,
                    image: output.image,
                    is_error: output.is_error,
                });
            }
            messages.push(Message {
                role: Role::User,
                content: results,
            });
        }
    }

    /// Execute one tool call: permission gate first (read-only tools pass
    /// unconditionally), then MCP routing or the built-in registry.
    async fn execute(&mut self, name: &str, input: Value) -> ToolOutput {
        if !self.registry.is_read_only(name) {
            let detail = ToolRegistry::call_detail(name, &input);
            if let Decision::Deny(reason) = self.gate.check(name, &detail) {
                return ToolOutput::error(format!("permission denied: {reason}"));
            }
        }
        if let Some((server, local)) = parse_mcp_name(name) {
            let Some(client) = self.mcp.get_mut(server) else {
                return ToolOutput::error(format!("MCP server {server:?} is not connected"));
            };
            return match client.call(local, input).await {
                Ok(output) => output,
                Err(error) => ToolOutput::error(format!("MCP call failed: {error}")),
            };
        }
        match self.registry.call_builtin(name, input).await {
            Some(output) => output,
            None => ToolOutput::error(format!("unknown tool {name:?}")),
        }
    }
}

/// Product prompt: identity and safety rules, skill summaries, today's date,
/// and the operator's configured appendix.
fn system_prompt(agent: &AgentConfig, skills: &[Skill], has_ass_mcp: bool) -> String {
    let mut prompt = String::from(SYSTEM_PROMPT);
    if has_ass_mcp {
        prompt.push_str(
            "\n\nThe `ass` MCP server is connected: its tools observe and operate the live \
             ASS compositor. Treat window, workspace, and Realm ids as opaque and short-lived.",
        );
    }
    if !skills.is_empty() {
        prompt.push_str("\n\nAvailable skills (read one with skill_read before following it):\n");
        prompt.push_str(&skills::summary_lines(skills).join("\n"));
    }
    if let Some(append) = &agent.system_prompt_append {
        prompt.push_str("\n\n");
        prompt.push_str(append);
    }
    let now = timestamp();
    if now.len() >= 8 {
        prompt.push_str(&format!(
            "\n\nToday is {}-{}-{}.",
            &now[..4],
            &now[4..6],
            &now[6..8]
        ));
    }
    prompt
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("stopped after reaching the {0}-turn limit")]
    MaxTurns(u32),
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use serde_json::json;

    use super::*;
    use crate::agent::config::PermissionsConfig;
    use crate::agent::provider::Response;

    struct MockProvider {
        responses: RefCell<VecDeque<Response>>,
    }

    impl MockProvider {
        fn new(responses: Vec<Response>) -> Self {
            Self {
                responses: RefCell::new(responses.into()),
            }
        }
    }

    impl Provider for MockProvider {
        async fn stream(
            &self,
            _request: &Request,
            on_event: &mut dyn FnMut(StreamEvent),
        ) -> Result<Response, ProviderError> {
            let response = self
                .responses
                .borrow_mut()
                .pop_front()
                .expect("scripted response");
            for block in &response.content {
                if let ContentBlock::Text(text) = block {
                    on_event(StreamEvent::TextDelta(text.clone()));
                }
            }
            Ok(response)
        }
    }

    fn text_response(text: &str) -> Response {
        Response {
            content: vec![ContentBlock::Text(text.to_string())],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 5,
                output_tokens: 3,
            },
        }
    }

    fn tool_response(id: &str, name: &str, input: Value) -> Response {
        Response {
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        }
    }

    fn agent(provider: MockProvider, permissions: &str) -> Agent<MockProvider> {
        let config: PermissionsConfig = toml::from_str(permissions).expect("permissions");
        Agent::assemble(
            provider,
            ToolRegistry::new(vec![]),
            BTreeMap::new(),
            PermissionGate::new(config, false),
            "test system".into(),
            "mock-model".into(),
            1024,
            32,
        )
    }

    #[tokio::test]
    async fn single_text_turn_completes() {
        let mut agent = agent(MockProvider::new(vec![text_response("all done")]), "");
        let mut messages = Vec::new();
        let mut events = Vec::new();
        let outcome = agent
            .run(&mut messages, "hi", &mut |event| events.push(event))
            .await
            .expect("run");
        assert_eq!(outcome.text, "all done");
        assert_eq!(outcome.turns, 1);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], Message::user("hi"));
        assert_eq!(messages[1].text(), "all done");
        assert_eq!(events, vec![AgentEvent::TextDelta("all done".into())]);
    }

    #[tokio::test]
    async fn tool_round_trip_executes_and_reports() {
        let mut agent = agent(
            MockProvider::new(vec![
                tool_response("t1", "bash", json!({"command": "echo fuji"})),
                text_response("ran it"),
            ]),
            "default = \"allow\"\n",
        );
        let mut messages = Vec::new();
        let outcome = agent
            .run(&mut messages, "run something", &mut |_| {})
            .await
            .expect("run");
        assert_eq!(outcome.text, "ran it");
        assert_eq!(outcome.turns, 2);
        // user, assistant(tool_use), user(tool_result), assistant(text)
        assert_eq!(messages.len(), 4);
        let result = &messages[2].content[0];
        let ContentBlock::ToolResult { text, is_error, .. } = result else {
            panic!("expected tool result");
        };
        assert!(!is_error);
        assert_eq!(text.trim_end(), "fuji");
    }

    #[tokio::test]
    async fn denied_tool_never_executes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nope.txt");
        let mut agent = agent(
            MockProvider::new(vec![
                tool_response(
                    "t1",
                    "write_file",
                    json!({"path": path.to_string_lossy(), "content": "x"}),
                ),
                text_response("blocked"),
            ]),
            "default = \"ask\"\nwrite_file = \"deny\"\n",
        );
        let mut messages = Vec::new();
        agent
            .run(&mut messages, "write", &mut |_| {})
            .await
            .expect("run");
        assert!(!path.exists());
        let ContentBlock::ToolResult { text, is_error, .. } = &messages[2].content[0] else {
            panic!("expected tool result");
        };
        assert!(is_error);
        assert!(text.contains("permission denied"), "{text}");
    }

    #[tokio::test]
    async fn read_only_tools_skip_the_permission_gate() {
        let mut agent = agent(
            MockProvider::new(vec![
                tool_response("t1", "glob", json!({"pattern": "*.rs"})),
                text_response("listed"),
            ]),
            "default = \"deny\"\n",
        );
        let mut messages = Vec::new();
        agent
            .run(&mut messages, "list", &mut |_| {})
            .await
            .expect("run");
        let ContentBlock::ToolResult { is_error, .. } = &messages[2].content[0] else {
            panic!("expected tool result");
        };
        assert!(!is_error, "read-only tools must bypass even a deny default");
    }

    #[tokio::test]
    async fn runaway_tool_loop_hits_the_turn_limit() {
        let mut agent = agent(
            MockProvider::new(vec![
                tool_response("t1", "glob", json!({"pattern": "*"})),
                tool_response("t2", "glob", json!({"pattern": "*"})),
                tool_response("t3", "glob", json!({"pattern": "*"})),
            ]),
            "",
        );
        agent.set_max_turns(2);
        let mut messages = Vec::new();
        let error = agent
            .run(&mut messages, "loop", &mut |_| {})
            .await
            .expect_err("max turns");
        assert!(matches!(error, AgentError::MaxTurns(2)));
    }
}
