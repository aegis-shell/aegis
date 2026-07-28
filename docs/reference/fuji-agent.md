# fuji Agent Reference

`fuji` is fuji's agent command line: a streaming chat client with built-in
file, shell, and image tools, stdio MCP connectivity, sessions, skills, and
a per-tool permission policy. It reaches Aegis exclusively through
`aegis-fuji-mcp`; see the [fuji Bridge Reference](fuji.md) for the desktop
tool contract.

## Commands

```text
fuji [chat] [--model <name>] [--max-turns <n>] [--yes]
fuji run <prompt...> [--model <name>] [--max-turns <n>] [--yes]
fuji resume <id|latest> [prompt...] [--model <name>] [--max-turns <n>] [--yes]
fuji print-config
fuji check
```

With no subcommand, `fuji` starts an interactive chat REPL on a fresh
session. `run` executes one prompt and prints the final answer; assistant
text streams to stdout while tool activity and diagnostics go to stderr.
`resume` loads a stored session and either runs the given prompt or enters
the REPL with the loaded history. `print-config` writes an annotated example
configuration to stdout. `check` validates configuration and connectivity
without calling the model.

The REPL understands `/help`, `/tools`, `/clear` (start a fresh session),
and `/quit`.

| Flag | Default | Description |
|------|---------|-------------|
| `--model <name>` | configured model | Override the provider model for this run. |
| `--max-turns <n>` | `32` | Override the agent loop turn limit. |
| `--yes`, `-y` | off | Auto-approve every permission prompt. |

| Exit status | Meaning |
|-------------|---------|
| `0` | The command completed. |
| `1` | Configuration, credential, provider, session, or MCP failure, or the turn limit was reached. |

`check` prints the resolved provider, endpoint, credential status, discovered
skill count, and every enabled MCP server with its tool count, and exits
non-zero when any of them fails.

## Configuration

The configuration file is `$XDG_CONFIG_HOME/fuji/config.toml` (override with
`FUJI_CONFIG`). Every section is optional; a missing file is valid and uses
the documented defaults.

### `[provider]`

| Key | Default | Description |
|-----|---------|-------------|
| `kind` | `"anthropic"` | `"anthropic"` or `"openai-compatible"`. |
| `model` | `"claude-sonnet-4-5"` | Model name passed to the provider. |
| `api_key_env` | per kind | Environment variable holding the credential: `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`. |
| `base_url` | per kind | Endpoint: `https://api.anthropic.com` or `https://api.openai.com/v1`. |
| `max_tokens` | `8192` | Response budget per request. |

Both providers stream over SSE. The OpenAI-compatible kind covers OpenAI,
DeepSeek, Qwen, and local endpoints that speak Chat Completions.

### `[agent]`

| Key | Default | Description |
|-----|---------|-------------|
| `max_turns` | `32` | Provider round-trips before the loop stops. |
| `system_prompt_append` | unset | Text appended to fuji's system prompt. |

### `[permissions]`

| Key | Default | Description |
|-----|---------|-------------|
| `default` | `"ask"` | Policy for tools without an entry: `allow`, `ask`, or `deny`. |
| `<tool name>` | unset | Per-tool override, e.g. `bash = "ask"` or `"mcp__aegis__realm_input" = "allow"`. |

`ask` prompts on the terminal (auto-answered by `--yes`); `deny` blocks the
call and reports it to the model. Read-only tools — `read_file`, `glob`,
`grep`, `read_image`, `skill_read`, and MCP tools annotated read-only —
never prompt.

### `[mcp.<name>]`

| Key | Default | Description |
|-----|---------|-------------|
| `command` | required | argv used to spawn the stdio server. |
| `enabled` | `true` | Disabled servers are skipped. |
| `read_only` | `false` | Informational flag for the operator. |
| `environment` | `{}` | Extra environment variables for the server. |

Each server's tools are namespaced as `mcp__<name>__<tool>`. Image content
in a tool result is forwarded to the model as image input.

### `[skills]`

| Key | Default | Description |
|-----|---------|-------------|
| `paths` | `[]` | Roots scanned for `*/SKILL.md` files. |

Skill names and descriptions appear in the system prompt; the model loads
full instructions through the `skill_read` tool. Frontmatter parsing covers
flat `name:` and `description:` scalars only.

## Built-in Tools

| Name | Read-only | Contract |
|------|-----------|----------|
| `read_file` | yes | UTF-8 file slice by 1-based `offset`/`limit` (max 2000 lines). |
| `write_file` | no | Create or fully overwrite, creating parents. |
| `edit_file` | no | Exact-string replacement; fails on zero or ambiguous matches without `replace_all`. |
| `glob` | yes | Up to 100 files matching a `**`-capable pattern. |
| `grep` | yes | Rust regex over file contents with optional include glob; skips binaries and `.git`. |
| `bash` | no | `bash -c` with a 120s default timeout (max 600s) and capped combined output. |
| `read_image` | yes | PNG/JPEG/GIF/WebP up to 20 MiB as model-visible image content. |
| `skill_read` | yes | Full instructions of one discovered skill by name. |

Paths are interpreted against fuji's working directory.

## Sessions

Each conversation is appended as JSONL to
`$XDG_DATA_HOME/fuji/sessions/<id>.jsonl`, where the id is
`YYYYMMDD-HHMMSS-<pid>` in UTC. `fuji run` prints its session id to stderr;
`fuji resume latest` continues the newest one. Sessions persist full message
history, including tool results.
