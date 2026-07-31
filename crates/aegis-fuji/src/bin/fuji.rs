use std::io::Write;
use std::process::ExitCode;

use aegis_fuji::agent::config::{ConfigError, FujiConfig};
use aegis_fuji::agent::mcp_client::McpClient;
use aegis_fuji::agent::provider::AnyProvider;
use aegis_fuji::agent::session::{Session, SessionError};
use aegis_fuji::agent::skills;
use aegis_fuji::agent::{Agent, AgentError, AgentEvent};
use clap::{Parser, Subcommand};

const SAMPLE_CONFIG: &str = "\
# fuji (宓姬) configuration — $XDG_CONFIG_HOME/fuji/config.toml
# A missing file is valid: defaults are anthropic + claude-sonnet-4-5.

[provider]
kind = \"anthropic\"                 # or \"openai-compatible\"
model = \"claude-sonnet-4-5\"
# api_key_env = \"ANTHROPIC_API_KEY\"  # default per kind
# base_url = \"https://api.anthropic.com\"  # default per kind
max_tokens = 8192

[agent]
max_turns = 32
# system_prompt_append = \"Always answer in Chinese.\"

[permissions]
default = \"ask\"                    # allow | ask | deny
# bash = \"ask\"
# \"mcp__aegis__realm_input\" = \"allow\"

# Aegis desktop bridge — build with: cargo build --release -p aegis-mcp
# [mcp.aegis]
# command = [\"/absolute/path/to/aegis-mcp\"]
# enabled = true
# read_only = false
# environment = { AEGIS_MCP_SCOPE = \"desktop-operator\" }

# [skills]
# paths = [\"/absolute/path/to/aegis/integrations/fuji/skills\"]
";

#[derive(Debug, Parser)]
#[command(
    name = "fuji",
    version,
    about = "fuji (宓姬) — the Aegis desktop agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run one prompt non-interactively and print the answer.
    Run {
        /// The prompt; joined with spaces.
        prompt: Vec<String>,
        #[command(flatten)]
        opts: RunOpts,
    },
    /// Chat interactively (default when no subcommand is given).
    Chat {
        #[command(flatten)]
        opts: RunOpts,
    },
    /// Resume a stored session; with a prompt it runs non-interactively,
    /// without one it enters chat with the loaded history.
    Resume {
        /// Session id from `fuji run` output, or "latest".
        id: String,
        /// Optional prompt; joined with spaces.
        prompt: Vec<String>,
        #[command(flatten)]
        opts: RunOpts,
    },
    /// Print an annotated example configuration.
    PrintConfig,
    /// Validate configuration, credentials, skills, and MCP connectivity.
    Check,
}

#[derive(Debug, Default, Clone, clap::Args)]
struct RunOpts {
    /// Override the configured model.
    #[arg(long)]
    model: Option<String>,
    /// Auto-approve every permission prompt.
    #[arg(long, short = 'y')]
    yes: bool,
    /// Override the configured turn limit.
    #[arg(long)]
    max_turns: Option<u32>,
}

#[tokio::main]
async fn main() -> ExitCode {
    match entry(Cli::parse()).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("fuji: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn entry(cli: Cli) -> Result<ExitCode, CliError> {
    match cli.command {
        Some(Command::PrintConfig) => {
            print!("{SAMPLE_CONFIG}");
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Check) => check().await,
        Some(Command::Run { prompt, opts }) => {
            let prompt = prompt.join(" ");
            if prompt.trim().is_empty() {
                return Err(CliError::Usage("run needs a non-empty prompt".into()));
            }
            let mut agent = build_agent(&opts).await?;
            let mut session = Session::create(&FujiConfig::data_dir())?;
            let result = run_turn(&mut agent, &mut session, prompt).await;
            agent.shutdown().await;
            result?;
            eprintln!("[session {}]", session.id());
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Resume { id, prompt, opts }) => {
            let mut agent = build_agent(&opts).await?;
            let mut session = Session::load(&FujiConfig::data_dir(), &id)?;
            let prompt = prompt.join(" ");
            if prompt.trim().is_empty() {
                chat(agent, session).await?;
            } else {
                let result = run_turn(&mut agent, &mut session, prompt).await;
                agent.shutdown().await;
                result?;
                eprintln!("[session {}]", session.id());
            }
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Chat { opts }) => {
            let agent = build_agent(&opts).await?;
            let session = Session::create(&FujiConfig::data_dir())?;
            chat(agent, session).await?;
            Ok(ExitCode::SUCCESS)
        }
        None => {
            let agent = build_agent(&RunOpts::default()).await?;
            let session = Session::create(&FujiConfig::data_dir())?;
            chat(agent, session).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn build_agent(opts: &RunOpts) -> Result<Agent<AnyProvider>, CliError> {
    let config = FujiConfig::load()?;
    let mut agent = Agent::from_config(&config, opts.yes, opts.model.clone()).await?;
    if let Some(max_turns) = opts.max_turns {
        agent.set_max_turns(max_turns);
    }
    Ok(agent)
}

/// One prompt/response cycle with live streaming to the terminal.
async fn run_turn(
    agent: &mut Agent<AnyProvider>,
    session: &mut Session,
    prompt: String,
) -> Result<(), CliError> {
    let start = session.messages.len();
    agent
        .run(&mut session.messages, prompt, &mut |event| match event {
            AgentEvent::TextDelta(text) => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            AgentEvent::ToolCall { name } => {
                eprintln!("\n→ {name}");
            }
        })
        .await?;
    println!();
    session.flush_from(start)?;
    Ok(())
}

async fn chat(mut agent: Agent<AnyProvider>, mut session: Session) -> Result<(), CliError> {
    let mut editor = rustyline::DefaultEditor::new().map_err(CliError::Readline)?;
    eprintln!("fuji — type /help for commands, /quit to exit");
    loop {
        match editor.readline("fuji> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match line {
                    "/quit" | "/exit" => break,
                    "/help" => eprintln!(
                        "/help   show this\n/tools  list available tools\n/clear  start a fresh session\n/quit   exit"
                    ),
                    "/tools" => println!("{}", agent.tool_names().join("\n")),
                    "/clear" => match Session::create(&FujiConfig::data_dir()) {
                        Ok(fresh) => {
                            session = fresh;
                            eprintln!("[new session {}]", session.id());
                        }
                        Err(error) => eprintln!("fuji: cannot start a new session: {error}"),
                    },
                    _ => {
                        let _ = editor.add_history_entry(line);
                        if let Err(error) =
                            run_turn(&mut agent, &mut session, line.to_string()).await
                        {
                            eprintln!("fuji: {error}");
                        }
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                eprintln!("(Ctrl-C: type /quit to exit)");
            }
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(error) => {
                agent.shutdown().await;
                return Err(CliError::Readline(error));
            }
        }
    }
    agent.shutdown().await;
    Ok(())
}

async fn check() -> Result<ExitCode, CliError> {
    let config = FujiConfig::load()?;
    let mut healthy = true;

    let resolved = config.provider.resolve();
    println!("provider: {:?} · model {}", resolved.kind, resolved.model);
    println!("endpoint: {}", resolved.base_url);
    if resolved.api_key().is_some() {
        println!("credentials: {} is set", resolved.api_key_env);
    } else {
        println!("credentials: {} is MISSING", resolved.api_key_env);
        healthy = false;
    }

    let discovered = skills::discover(&config.skills.paths);
    println!("skills: {} discovered", discovered.len());

    for (name, server) in &config.mcp {
        if !server.enabled {
            println!("mcp {name}: disabled");
            continue;
        }
        match McpClient::spawn(name, server).await {
            Ok(mut client) => {
                println!("mcp {name}: connected, {} tools", client.tools().len());
                client.shutdown().await;
            }
            Err(error) => {
                println!("mcp {name}: FAILED to start: {error}");
                healthy = false;
            }
        }
    }

    Ok(if healthy {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("terminal input failed: {0}")]
    Readline(#[from] rustyline::error::ReadlineError),
    #[error("{0}")]
    Usage(String),
}
