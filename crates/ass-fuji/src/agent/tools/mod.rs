//! Tool execution: the built-in file/shell/image tools plus the registry
//! that merges them with namespaced MCP tool specs for the model.

mod builtin;

use std::collections::BTreeMap;
use std::pin::Pin;

use serde_json::Value;

use crate::agent::provider::{ImageData, ToolSpec};
use crate::agent::skills::Skill;

/// Result of one tool call, rendered back into the conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub image: Option<ImageData>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image: None,
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            text: message.into(),
            image: None,
            is_error: true,
        }
    }

    pub fn image(text: impl Into<String>, image: ImageData) -> Self {
        Self {
            text: text.into(),
            image: Some(image),
            is_error: false,
        }
    }
}

/// One executable tool. `call` returns a boxed future so the registry can
/// hold heterogeneous tools behind `dyn`.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn read_only(&self) -> bool {
        false
    }
    fn call<'a>(
        &'a self,
        arguments: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ToolOutput> + Send + 'a>>;
}

/// Built-in tools plus registered MCP tool specs.
pub struct ToolRegistry {
    builtin: Vec<Box<dyn Tool>>,
    mcp_specs: Vec<ToolSpec>,
    mcp_read_only: BTreeMap<String, bool>,
}

impl ToolRegistry {
    /// All built-ins, including `skill_read` bound to the discovered skills.
    pub fn new(skills: Vec<Skill>) -> Self {
        Self {
            builtin: builtin::all(skills),
            mcp_specs: Vec::new(),
            mcp_read_only: BTreeMap::new(),
        }
    }

    /// Register one server's namespaced specs and read-only hints.
    pub fn register_mcp_tools(&mut self, tools: Vec<(ToolSpec, bool)>) {
        for (spec, read_only) in tools {
            self.mcp_read_only.insert(spec.name.clone(), read_only);
            self.mcp_specs.push(spec);
        }
    }

    /// Every model-facing spec: built-ins first, then MCP tools.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.builtin
            .iter()
            .map(|tool| ToolSpec {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                input_schema: tool.input_schema(),
            })
            .chain(self.mcp_specs.iter().cloned())
            .collect()
    }

    pub fn is_read_only(&self, name: &str) -> bool {
        if let Some(tool) = self.builtin.iter().find(|tool| tool.name() == name) {
            return tool.read_only();
        }
        self.mcp_read_only.get(name).copied().unwrap_or(false)
    }

    /// Execute a built-in tool; `None` when the name is not built in.
    pub async fn call_builtin(&self, name: &str, arguments: Value) -> Option<ToolOutput> {
        let tool = self.builtin.iter().find(|tool| tool.name() == name)?;
        Some(tool.call(arguments).await)
    }

    /// One-line summary of what a call touches, for permission prompts.
    pub fn call_detail(name: &str, arguments: &Value) -> String {
        let key = match name {
            "bash" => arguments["command"].as_str().map(str::to_string),
            "read_file" | "write_file" | "edit_file" | "read_image" => {
                arguments["path"].as_str().map(str::to_string)
            }
            _ => None,
        };
        key.unwrap_or_else(|| {
            let text = arguments.to_string();
            text.chars().take(160).collect()
        })
    }
}

/// Split `mcp__<server>__<tool>` into `(server, tool)`.
pub fn parse_mcp_name(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("mcp__")?.split_once("__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_names_split_on_the_double_underscore_boundary() {
        assert_eq!(
            parse_mcp_name("mcp__ass__realm_capture"),
            Some(("ass", "realm_capture"))
        );
        assert_eq!(parse_mcp_name("mcp__ass__a__b"), Some(("ass", "a__b")));
        assert_eq!(parse_mcp_name("bash"), None);
        assert_eq!(parse_mcp_name("mcp__lonely"), None);
    }
}
