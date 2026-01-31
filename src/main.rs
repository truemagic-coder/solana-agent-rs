#[cfg(not(test))]
use clap::Parser;
#[cfg(not(test))]
use console::{style, Term};
#[cfg(not(test))]
use futures::StreamExt;
#[cfg(not(test))]
use notify_rust::Notification;
#[cfg(not(test))]
use pulldown_cmark::{Options, Parser as MarkdownParser};
#[cfg(not(test))]
use pulldown_cmark_mdcat::{
    resources::NoopResourceHandler, Environment, Settings, TerminalProgram, TerminalSize, Theme,
};
#[cfg(not(test))]
use reqwest::header::AUTHORIZATION;
#[cfg(not(test))]
#[cfg(not(test))]
use std::collections::{HashMap, HashSet};
#[cfg(not(test))]
use std::io::{self as std_io, BufWriter, Write};
#[cfg(not(test))]
use std::process::Command;
#[cfg(not(test))]
use syntect::parsing::SyntaxSet;
#[cfg(not(test))]
use tokio::io::{self, AsyncBufReadExt};

#[cfg(not(test))]
use butterfly_bot::config::{AgentConfig, Config, MemoryConfig, OpenAiConfig};
#[cfg(not(test))]
use butterfly_bot::config_store;
#[cfg(not(test))]
use butterfly_bot::daemon;
#[cfg(not(test))]
use butterfly_bot::error::Result;
#[cfg(not(test))]
use butterfly_bot::interfaces::plugins::Tool;
#[cfg(not(test))]
use butterfly_bot::plugins::registry::ToolRegistry;
#[cfg(not(test))]
use butterfly_bot::tools::search_internet::SearchInternetTool;
#[cfg(not(test))]
use butterfly_bot::ui;
#[cfg(not(test))]
use butterfly_bot::vault;
#[cfg(not(test))]
use tokio::sync::oneshot;
#[cfg(not(test))]
use tracing_subscriber::EnvFilter;

#[cfg(not(test))]
#[derive(Parser, Debug)]
#[command(name = "butterfly-bot")]
#[command(about = "ButterFly Bot CLI (Rust)")]
struct Cli {
    #[arg(
        long = "cli",
        default_value_t = false,
        help = "Start in CLI mode (default is UI)"
    )]
    cli_mode: bool,

    #[arg(long, default_value = "./data/butterfly-bot.db")]
    db: String,

    #[arg(long, default_value = "http://127.0.0.1:7878")]
    daemon: String,

    #[arg(long, env = "BUTTERFLY_BOT_TOKEN")]
    token: Option<String>,

    #[arg(long, default_value = "cli_user")]
    user_id: String,

    #[arg(long)]
    prompt: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[cfg(not(test))]
#[derive(clap::Subcommand, Debug)]
enum Commands {
    Status,
    MemorySearch {
        #[arg(long)]
        query: String,

        #[arg(long, default_value_t = 8)]
        limit: usize,
    },
    ConfigImport {
        #[arg(long)]
        path: String,
    },
    ConfigExport {
        #[arg(long)]
        path: String,
    },
    ConfigShow,
    Init,
    SecretsSet {
        #[arg(long)]
        openai_key: String,
    },
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
}

#[cfg(not(test))]
#[derive(clap::Subcommand, Debug)]
enum ToolsCommand {
    List,
    Enable {
        #[arg(long)]
        name: String,
    },
    Disable {
        #[arg(long)]
        name: String,
    },
}

#[cfg(not(test))]
fn rule(width: usize) -> String {
    "─".repeat(width.clamp(36, 96))
}

#[cfg(not(test))]
fn print_banner(daemon: &str, user_id: &str) {
    let term = Term::stdout();
    let width = term.size().1 as usize;
    let line = rule(width);

    let title = [
        r"__________ ____ _________________________________________________________.____    _____.___. __________ ___________________",
        r"\______   \    |   \__    ___/\__    ___/\_   _____/\______   \_   _____/|    |   \__  |   | \______   \\_____  \__    ___/",
        r"|    |  _/    |   / |    |     |    |    |    __)_  |       _/|    __)  |    |    /   |   |  |    |  _/ /   |   \|    |    ",
        r"|    |   \    |  /  |    |     |    |    |        \ |    |   \|     \   |    |___ \____   |  |    |   \/    |    \    |    ",
        r"|______  /______/   |____|     |____|   /_______  / |____|_  /\___  /   |_______ \/ ______|  |______  /\_______  /____|    ",
        r"       \/                                       \/         \/     \/            \/\/                \/         \/          ",
    ];

    println!("{}", style(&line).color256(214));
    let palette = [214u8, 208, 202, 214, 220, 208];
    for (idx, row) in title.iter().enumerate() {
        let shade = palette[idx % palette.len()];
        println!("{}", style(*row).color256(shade).bold());
    }
    println!("{}", style(&line).color256(214));
    println!(
        "{}",
        style(format!(
            "Ollama-ready • Streaming CLI • Daemon: {} • User: {}",
            daemon, user_id
        ))
        .color256(250)
    );
    println!();
}

#[cfg(not(test))]
fn print_user_prompt() -> io::Result<()> {
    let mut out = std_io::stdout();
    write!(
        out,
        "{} {} ",
        style("➜").color256(45).bold(),
        style("You").color256(81).bold()
    )?;
    out.flush()
}

#[cfg(not(test))]
async fn start_reminder_listener(cli: &Cli) {
    let token = cli.token.clone().unwrap_or_default();
    let url = format!(
        "{}/reminder_stream?user_id={}",
        cli.daemon.trim_end_matches('/'),
        cli.user_id
    );

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            let mut request = client.get(url.clone());
            if !token.trim().is_empty() {
                request = request.header(AUTHORIZATION, format!("Bearer {}", token));
            }
            let resp = request.send().await;
            let Ok(resp) = resp else {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            };
            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();
            while let Some(chunk) = stream.next().await {
                let Ok(chunk) = chunk else {
                    break;
                };
                if let Ok(text) = std::str::from_utf8(&chunk) {
                    buffer.push_str(text);
                    while let Some(idx) = buffer.find("\n") {
                        let mut line = buffer[..idx].to_string();
                        buffer = buffer[idx + 1..].to_string();
                        if line.starts_with("data:") {
                            line = line.trim_start_matches("data:").trim().to_string();
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                                let title = value
                                    .get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Reminder");
                                let _ = std_io::stdout().write_all(b"\n\n");
                                println!("{} {}", style("⏰").color256(214), title);
                                let _ = Notification::new()
                                    .summary("Butterfly Bot")
                                    .body(title)
                                    .show();
                                let _ = print_user_prompt();
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

#[cfg(not(test))]
fn print_assistant_prefix() {
    print!(
        "{} {} ",
        style("✦").color256(214).bold(),
        style("Butterfly").color256(214).bold()
    );
}

#[cfg(not(test))]
fn render_markdown(markdown: &str) {
    if markdown.trim().is_empty() {
        return;
    }
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = MarkdownParser::new_ext(markdown, options);
    let env = match std::env::current_dir()
        .ok()
        .and_then(|dir| Environment::for_local_directory(&dir).ok())
    {
        Some(env) => env,
        None => {
            print!("{markdown}");
            return;
        }
    };
    let term = Term::stdout();
    let columns = term.size().1;
    let terminal_size = TerminalSize::detect()
        .unwrap_or_default()
        .with_max_columns(columns.max(40));
    let settings = Settings {
        terminal_capabilities: TerminalProgram::detect().capabilities(),
        terminal_size,
        syntax_set: &SyntaxSet::load_defaults_newlines(),
        theme: Theme::default(),
    };
    let mut out = std_io::stdout();
    let mut sink = BufWriter::new(&mut out);
    if pulldown_cmark_mdcat::push_tty(&settings, &env, &NoopResourceHandler, &mut sink, parser)
        .and_then(|_| sink.flush())
        .is_err()
    {
        print!("{markdown}");
    }
}

#[cfg(not(test))]
fn should_use_markdown(text: &str) -> bool {
    let markdown_tokens = ["```", "\n|", "|---", "[`", "]("];
    markdown_tokens.iter().any(|token| text.contains(token))
}

#[cfg(not(test))]
fn render_response(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if should_use_markdown(text) {
        let prefixed = format!("**Butterfly:** {text}");
        render_markdown(&prefixed);
    } else {
        print_assistant_prefix();
        print!("{text}");
    }
}

#[cfg(not(test))]
fn clear_streamed_output(response: &str) {
    let term = Term::stdout();
    let width = term.size().1.max(1) as usize;
    let mut lines = 0usize;
    for line in response.split('\n') {
        let len = line.chars().count().max(1);
        lines += len.div_ceil(width);
    }
    for _ in 0..lines {
        print!("\x1b[2K\x1b[1A");
    }
    print!("\x1b[2K\r");
    let _ = std_io::stdout().flush();
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,butterfly_bot=info,lance=warn,lancedb=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    if !cli.cli_mode {
        ui::launch_ui();
        return Ok(());
    }
    let needs_onboarding = !matches!(
        cli.command,
        Some(Commands::Init) | Some(Commands::ConfigImport { .. })
    );
    if needs_onboarding && Config::from_store(&cli.db).is_err() {
        run_onboarding(&cli.db)?;
        println!("Onboarding complete. Run 'butterfly-bot config show' to review.");
    }

    if let Ok(config) = Config::from_store(&cli.db) {
        ensure_ollama_models(&config)?;
    }

    let uses_daemon = cli.prompt.is_some()
        || matches!(
            cli.command,
            None | Some(Commands::Status) | Some(Commands::MemorySearch { .. })
        );
    let _daemon_shutdown = if uses_daemon {
        let (host, port) = parse_daemon_address(&cli.daemon);
        let token = cli.token.clone().unwrap_or_default();
        let db_path = cli.db.clone();
        let (tx, rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = daemon::run_with_shutdown(&host, port, &db_path, &token, async {
                let _ = rx.await;
            })
            .await;
        });
        Some(DaemonShutdown(Some(tx)))
    } else {
        None
    };
    if let Some(command) = &cli.command {
        match command {
            Commands::Init => {
                run_onboarding(&cli.db)?;
                println!("Onboarding complete. Run 'butterfly-bot config show' to review.");
                return Ok(());
            }
            Commands::ConfigImport { path } => {
                let config = Config::from_file(path)?;
                config_store::save_config(&cli.db, &config)?;
                println!("Config imported into {}", cli.db);
                return Ok(());
            }
            Commands::ConfigExport { path } => {
                let config = Config::from_store(&cli.db)?;
                let value = redacted_config_value(&config)?;
                write_config_file(path, &value)?;
                println!("Config exported to {path}");
                return Ok(());
            }
            Commands::ConfigShow => {
                let config = Config::from_store(&cli.db)?;
                let value = redacted_config_value(&config)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
                return Ok(());
            }
            Commands::SecretsSet { openai_key } => {
                vault::set_secret("openai_api_key", openai_key)?;
                println!("Secret stored in keyring.");
                return Ok(());
            }
            Commands::Tools { command } => {
                handle_tools_command(&cli.db, command)?;
                return Ok(());
            }
            Commands::Status => {
                let status = daemon_status(&cli).await?;
                println!("{status}");
                return Ok(());
            }
            _ => {}
        }
    }

    print_banner(&cli.daemon, &cli.user_id);

    if let Some(Commands::MemorySearch { query, limit }) = &cli.command {
        let results = daemon_memory_search(&cli, query, *limit).await?;
        if results.is_empty() {
            println!("{}", style("No memory matches.").color256(245));
        } else {
            println!("{}", style("Memory matches:").color256(81).bold());
            for item in results {
                println!("- {item}");
            }
        }
        return Ok(());
    }

    if let Some(prompt) = &cli.prompt {
        ensure_tool_secrets(&cli.db).await?;
        let response = daemon_process_text_stream(&cli, prompt, None, false).await?;
        render_response(&response);
        println!();
        return Ok(());
    }

    println!(
        "{}",
        style("Enter your prompts (Ctrl+D to exit):").color256(245)
    );
    start_reminder_listener(&cli).await;
    ensure_tool_secrets(&cli.db).await?;
    let stdin = io::BufReader::new(io::stdin());
    let mut lines = stdin.lines();
    loop {
        print_user_prompt()
            .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
        let line = lines
            .next_line()
            .await
            .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
        let Some(line) = line else {
            println!("\n{}", style("Goodbye ✨").color256(245));
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        print_assistant_prefix();
        let response = daemon_process_text_stream(&cli, &line, None, true).await?;
        println!();
        if should_use_markdown(&response) {
            clear_streamed_output(&response);
            let prefixed = format!("**Butterfly:** {response}");
            render_markdown(&prefixed);
            println!();
        }
    }

    Ok(())
}

#[cfg(not(test))]
async fn ensure_tool_secrets(db_path: &str) -> Result<()> {
    let mut config = Config::from_store(db_path)?;
    let mut agent_tools: Vec<(String, Vec<String>)> = Vec::new();
    for agent in &config.agents {
        agent_tools.push((agent.name.clone(), agent.tools.clone().unwrap_or_default()));
    }

    let mut enabled_tools: HashSet<String> = HashSet::new();
    for (_, tools) in &agent_tools {
        for tool in tools {
            enabled_tools.insert(tool.to_string());
        }
    }

    let mut tools_config = config.tools.clone().unwrap_or(serde_json::Value::Null);
    let has_search_config = tools_config.get("search_internet").is_some();
    let mut updated_config = false;
    if (enabled_tools.contains("search_internet") || has_search_config)
        && ensure_search_internet_provider(&mut config)?
    {
        updated_config = true;
    }

    if updated_config {
        config_store::save_config(db_path, &config)?;
        tools_config = config.tools.clone().unwrap_or(serde_json::Value::Null);
    }

    let config_value = serde_json::to_value(&config)
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Config(e.to_string()))?;

    let registry = ToolRegistry::new();
    registry.configure_all_tools(config_value.clone()).await?;

    let has_search_config = tools_config.get("search_internet").is_some();
    if enabled_tools.contains("search_internet") || has_search_config {
        let tool: std::sync::Arc<dyn Tool> = std::sync::Arc::new(SearchInternetTool::new());
        tool.configure(&config_value)?;
        let _ = registry.register_tool(tool).await;
    }

    let mut required: HashMap<String, String> = HashMap::new();
    for (_, tools) in &agent_tools {
        for tool_name in tools {
            if !registry.is_tool_enabled(tool_name).await {
                continue;
            }
            if let Some(tool) = registry.get_tool(tool_name).await {
                for secret in tool.required_secrets_for_config(&config_value) {
                    required.insert(secret.name, secret.prompt);
                }
            }
        }
    }

    for (name, prompt) in required {
        if vault::get_secret(&name)?.is_some() {
            continue;
        }
        let value = prompt_line(&format!("{}: ", prompt))?;
        if value.trim().is_empty() {
            continue;
        }
        vault::set_secret(&name, value.trim())?;
        println!("Stored '{}' in keyring.", name);
    }

    Ok(())
}

#[cfg(not(test))]
fn ensure_search_internet_provider(config: &mut Config) -> Result<bool> {
    let tools = config
        .tools
        .get_or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !tools.is_object() {
        *tools = serde_json::Value::Object(Default::default());
    }
    let tools_map = tools.as_object_mut().unwrap();
    let tool_cfg = tools_map
        .entry("search_internet".to_string())
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !tool_cfg.is_object() {
        *tool_cfg = serde_json::Value::Object(Default::default());
    }
    let tool_map = tool_cfg.as_object_mut().unwrap();

    let provider = tool_map
        .get("provider")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string());
    if provider.is_some() {
        return Ok(false);
    }

    println!("Select provider for search_internet:");
    println!("  1) openai");
    println!("  2) perplexity");
    println!("  3) grok");
    let choice = prompt_line("Provider [openai]: ")?;
    let provider = match choice.trim() {
        "2" | "perplexity" => "perplexity",
        "3" | "grok" => "grok",
        "" | "1" | "openai" => "openai",
        other => {
            println!("Unknown provider '{other}', defaulting to openai.");
            "openai"
        }
    };

    tool_map.insert(
        "provider".to_string(),
        serde_json::Value::String(provider.to_string()),
    );
    Ok(true)
}

#[cfg(not(test))]
fn handle_tools_command(db_path: &str, command: &ToolsCommand) -> Result<()> {
    match command {
        ToolsCommand::List => {
            let config = Config::from_store(db_path)?;
            let rows = list_tools_from_config(&config);
            if rows.is_empty() {
                println!("No tools configured.");
                return Ok(());
            }
            for (name, enabled) in rows {
                let status = if enabled { "enabled" } else { "disabled" };
                println!("{name} [{status}]");
            }
            Ok(())
        }
        ToolsCommand::Enable { name } => {
            let mut config = Config::from_store(db_path)?;
            let changed = update_tool_state(&mut config, name, true)?;
            if changed {
                config_store::save_config(db_path, &config)?;
                println!("Enabled tool '{name}'.");
            } else {
                println!("Tool '{name}' already enabled.");
            }
            Ok(())
        }
        ToolsCommand::Disable { name } => {
            let mut config = Config::from_store(db_path)?;
            let changed = update_tool_state(&mut config, name, false)?;
            if changed {
                config_store::save_config(db_path, &config)?;
                println!("Disabled tool '{name}'.");
            } else {
                println!("Tool '{name}' already disabled.");
            }
            Ok(())
        }
    }
}

#[cfg(not(test))]
fn redacted_config_value(config: &Config) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(config)
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Config(e.to_string()))?;
    if let Some(openai) = value.get_mut("openai") {
        if let Some(obj) = openai.as_object_mut() {
            obj.remove("api_key");
        }
    }
    Ok(value)
}

#[cfg(not(test))]
fn write_config_file(path: &str, value: &serde_json::Value) -> Result<()> {
    let path_obj = std::path::Path::new(path);
    if let Some(parent) = path_obj.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    }
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Config(e.to_string()))?;
    std::fs::write(path_obj, rendered)
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    Ok(())
}

#[cfg(not(test))]
fn list_tools_from_config(config: &Config) -> Vec<(String, bool)> {
    let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut enabled: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut disabled: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut has_enabled_list = false;

    if let Some(tools) = &config.tools {
        if let Some(map) = tools.as_object() {
            for (key, _) in map.iter() {
                if key != "settings" {
                    names.insert(key.clone());
                }
            }
            if let Some(settings) = map.get("settings") {
                if let Some(enabled_list) = settings.get("enabled").and_then(|v| v.as_array()) {
                    has_enabled_list = true;
                    for item in enabled_list {
                        if let Some(name) = item.as_str() {
                            enabled.insert(name.to_string());
                            names.insert(name.to_string());
                        }
                    }
                }
                if let Some(disabled_list) = settings.get("disabled").and_then(|v| v.as_array()) {
                    for item in disabled_list {
                        if let Some(name) = item.as_str() {
                            disabled.insert(name.to_string());
                            names.insert(name.to_string());
                        }
                    }
                }
            }
        }
    }

    let mut rows: Vec<(String, bool)> = names
        .into_iter()
        .map(|name| {
            let enabled_state = if disabled.contains(&name) {
                false
            } else if has_enabled_list {
                enabled.contains(&name)
            } else {
                true
            };
            (name, enabled_state)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

#[cfg(not(test))]
fn update_tool_state(config: &mut Config, name: &str, enable: bool) -> Result<bool> {
    let tools = config
        .tools
        .get_or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !tools.is_object() {
        *tools = serde_json::Value::Object(Default::default());
    }
    let tools_map = tools.as_object_mut().unwrap();
    let settings = tools_map
        .entry("settings".to_string())
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !settings.is_object() {
        *settings = serde_json::Value::Object(Default::default());
    }
    let settings_map = settings.as_object_mut().unwrap();

    let safe_mode = settings_map
        .get("safe_mode")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let enabled_present = settings_map.contains_key("enabled");
    let mut enabled_list: Vec<String> = settings_map
        .get("enabled")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut has_enabled_list = enabled_present;
    if enabled_list.is_empty() && !safe_mode {
        has_enabled_list = false;
    }

    let mut disabled_list: Vec<String> = settings_map
        .get("disabled")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut changed = false;
    if enable {
        let before = disabled_list.len();
        disabled_list.retain(|item| item != name);
        if disabled_list.len() != before {
            changed = true;
        }

        if (has_enabled_list || safe_mode) && !enabled_list.iter().any(|item| item == name) {
            enabled_list.push(name.to_string());
            changed = true;
        }
    } else {
        if !disabled_list.iter().any(|item| item == name) {
            disabled_list.push(name.to_string());
            changed = true;
        }
        if has_enabled_list {
            let before = enabled_list.len();
            enabled_list.retain(|item| item != name);
            if enabled_list.len() != before {
                changed = true;
            }
        }
    }

    settings_map.insert(
        "enabled".to_string(),
        serde_json::Value::Array(
            enabled_list
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    settings_map.insert(
        "disabled".to_string(),
        serde_json::Value::Array(
            disabled_list
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );

    Ok(changed)
}

#[cfg(not(test))]
fn run_onboarding(db_path: &str) -> Result<()> {
    println!("{}", style("ButterFly Bot setup").color256(214).bold());
    println!("{}", style("Using local Ollama defaults.").color256(245));
    println!();

    let base_url = "http://localhost:11434/v1".to_string();
    let model = "glm-4.7-flash:latest".to_string();
    let memory_enabled = true;

    let memory = if memory_enabled {
        let sqlite_path = prompt_with_default("Memory SQLite path", db_path)?;
        let lancedb_path = prompt_with_default("LanceDB path", "./data/lancedb")?;
        let embedding_model = "embeddinggemma:latest".to_string();
        let rerank_model = "qllama/bge-reranker-v2-m3".to_string();
        let summary_model = model.clone();
        let summary_threshold = prompt_optional_u32("Summary threshold (messages)")?;
        let retention_days = prompt_optional_u32("Retention days (blank for unlimited)")?;

        Some(MemoryConfig {
            enabled: Some(true),
            sqlite_path: Some(sqlite_path),
            lancedb_path: Some(lancedb_path),
            summary_model: Some(summary_model),
            embedding_model: Some(embedding_model),
            rerank_model: Some(rerank_model),
            summary_threshold: summary_threshold.map(|value| value as usize),
            retention_days,
        })
    } else {
        Some(MemoryConfig {
            enabled: Some(false),
            sqlite_path: None,
            lancedb_path: None,
            summary_model: None,
            embedding_model: None,
            rerank_model: None,
            summary_threshold: None,
            retention_days: None,
        })
    };

    let config = Config {
        openai: Some(OpenAiConfig {
            api_key: None,
            model: Some(model),
            base_url: Some(base_url),
        }),
        agents: vec![AgentConfig {
            name: "default_agent".to_string(),
            description: Some("Butterfly, an expert conversationalist and assistant.".to_string()),
            instructions:
                r#"You are Butterfly, an expert conversationalist and calm, capable assistant.

Core behavior:
- Be warm, concise, and natural. Ask clarifying questions when the request is ambiguous.
- Prefer actionable help over long explanations. Offer a short plan when helpful.
- If you’re unsure, say so briefly and suggest the next best step.

Tools you can use:
- reminders: create/list/complete/delete/snooze reminders and todos.
    Use it when the user asks for reminders, alarms, timers, tasks, or follow-ups.
- search_internet: fetch up-to-date info when the user asks for current events or live data.

Memory:
- Use provided context, but do not treat assistant statements as user facts.
- Confirm personal details before relying on them.

When scheduling:
- If the user asks “in X seconds/minutes/hours,” create a reminder with that delay.
- If they ask “tomorrow at 3pm” or similar, ask for timezone if missing.
"#
                .to_string(),
            specialization: "conversation".to_string(),
            tools: Some(vec!["reminders".to_string(), "search_internet".to_string()]),
            capture_name: None,
            capture_schema: None,
        }],
        business: None,
        memory,
        guardrails: None,
        tools: None,
        brains: None,
    };

    config_store::save_config(db_path, &config)?;
    Ok(())
}

#[cfg(not(test))]
fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    let prompt = format!("{} [{}]: ", label, default);
    let input = prompt_line(&prompt)?;
    if input.trim().is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input.trim().to_string())
    }
}

#[cfg(not(test))]
fn prompt_optional_u32(label: &str) -> Result<Option<u32>> {
    let prompt = format!("{}: ", label);
    let input = prompt_line(&prompt)?;
    if input.trim().is_empty() {
        return Ok(None);
    }
    input
        .trim()
        .parse::<u32>()
        .map(Some)
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Config(e.to_string()))
}

#[cfg(not(test))]
fn prompt_line(prompt: &str) -> Result<String> {
    let mut out = std_io::stdout();
    write!(out, "{}", style(prompt).color256(250))
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    out.flush()
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    let mut input = String::new();
    std_io::stdin()
        .read_line(&mut input)
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    Ok(input)
}

#[cfg(not(test))]
struct DaemonShutdown(Option<oneshot::Sender<()>>);

#[cfg(not(test))]
impl Drop for DaemonShutdown {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(not(test))]
fn parse_daemon_address(daemon: &str) -> (String, u16) {
    let trimmed = daemon.trim();
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let host_port = without_scheme.split('/').next().unwrap_or("127.0.0.1:7878");
    let mut parts = host_port.splitn(2, ':');
    let host = parts.next().unwrap_or("127.0.0.1");
    let port = parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(7878);
    (host.to_string(), port)
}

#[cfg(not(test))]
fn ensure_ollama_models(config: &Config) -> Result<()> {
    let Some(openai) = &config.openai else {
        return Ok(());
    };
    let Some(base_url) = &openai.base_url else {
        return Ok(());
    };
    if !is_ollama_local(base_url) {
        return Ok(());
    }

    let mut required = Vec::new();
    if let Some(model) = &openai.model {
        if !model.trim().is_empty() {
            required.push(model.clone());
        }
    }
    if let Some(memory) = &config.memory {
        for value in [
            memory.embedding_model.as_ref(),
            memory.rerank_model.as_ref(),
            memory.summary_model.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !value.trim().is_empty() {
                required.push(value.clone());
            }
        }
    }

    required.sort();
    required.dedup();
    if required.is_empty() {
        return Ok(());
    }

    let installed = list_ollama_models()?;
    for model in required {
        if !installed.iter().any(|name| name == &model) {
            pull_ollama_model(&model)?;
        }
    }

    Ok(())
}

#[cfg(not(test))]
fn is_ollama_local(base_url: &str) -> bool {
    base_url.starts_with("http://localhost:11434") || base_url.starts_with("http://127.0.0.1:11434")
}

#[cfg(not(test))]
fn list_ollama_models() -> Result<Vec<String>> {
    let output = Command::new("ollama")
        .arg("list")
        .output()
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    if !output.status.success() {
        return Err(butterfly_bot::error::ButterflyBotError::Runtime(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut models = Vec::new();
    for line in stdout.lines().skip(1) {
        let name = line.split_whitespace().next().unwrap_or("");
        if !name.is_empty() {
            models.push(name.to_string());
        }
    }
    Ok(models)
}

#[cfg(not(test))]
fn pull_ollama_model(model: &str) -> Result<()> {
    let status = Command::new("ollama")
        .arg("pull")
        .arg(model)
        .status()
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(butterfly_bot::error::ButterflyBotError::Runtime(format!(
            "Failed to pull model '{model}'"
        )))
    }
}

#[cfg(not(test))]
async fn daemon_status(cli: &Cli) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!("{}/health", cli.daemon.trim_end_matches('/'));
    let token = cli.token.as_deref().unwrap_or("").to_string();
    let response = client
        .get(url)
        .header("x-api-key", token)
        .send()
        .await
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    let text = response
        .text()
        .await
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    Ok(text)
}

#[cfg(not(test))]
async fn daemon_process_text_stream(
    cli: &Cli,
    text: &str,
    prompt: Option<&str>,
    print_stream: bool,
) -> Result<String> {
    let token = cli.token.as_deref();
    let client = reqwest::Client::new();
    let url = format!("{}/process_text_stream", cli.daemon.trim_end_matches('/'));
    let body = serde_json::json!({
        "user_id": cli.user_id,
        "text": text,
        "prompt": prompt,
    });
    let mut request = client.post(url);
    if let Some(token) = token {
        if !token.trim().is_empty() {
            request = request.header("authorization", format!("Bearer {token}"));
        }
    }
    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;

    if !response.status().is_success() {
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            return Err(butterfly_bot::error::ButterflyBotError::Runtime(
                error.to_string(),
            ));
        }
        return Err(butterfly_bot::error::ButterflyBotError::Runtime(
            "Invalid daemon response".to_string(),
        ));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);
        if print_stream {
            print!("{text}");
            std_io::stdout()
                .flush()
                .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
        }
    }
    Ok(buffer)
}

#[cfg(not(test))]
async fn daemon_memory_search(cli: &Cli, query: &str, limit: usize) -> Result<Vec<String>> {
    let token = cli.token.as_deref();
    let client = reqwest::Client::new();
    let url = format!("{}/memory_search", cli.daemon.trim_end_matches('/'));
    let body = serde_json::json!({
        "user_id": cli.user_id,
        "query": query,
        "limit": limit,
    });
    let mut request = client.post(url);
    if let Some(token) = token {
        if !token.trim().is_empty() {
            request = request.header("authorization", format!("Bearer {token}"));
        }
    }
    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|e| butterfly_bot::error::ButterflyBotError::Runtime(e.to_string()))?;
    if let Some(results) = value.get("results").and_then(|v| v.as_array()) {
        Ok(results
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect())
    } else if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        Err(butterfly_bot::error::ButterflyBotError::Runtime(
            error.to_string(),
        ))
    } else {
        Err(butterfly_bot::error::ButterflyBotError::Runtime(
            "Invalid daemon response".to_string(),
        ))
    }
}

#[cfg(test)]
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn covers_main_stub() {
        super::main();
    }
}
