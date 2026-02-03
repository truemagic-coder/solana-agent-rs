## Butterfly Bot

Butterfly Bot is a desktop app for chatting with your personal AI assistant. It includes reminders, memory, tool integrations, and streaming responses in a polished UI. The codebase still exposes a Rust library for building bots, but the primary focus is the app experience.

## Highlights

- Modern desktop UI (Dioxus) with streaming chat using local Ollama models
- Reminders and notifications both in chat and OS notifications
- Optional long-term memory using embedded local storage of SQLCipher and LanceDB
- Agent tool integrations with live UI events
- Config and secrets managed from the config screen

## Privacy & Security & Always On

- Run locally with Ollama to keep requests and model inference private on your machine.
- Designed for always-on use with unlimited token use (local inference) and customized wakeup and task intervals.
- Conversation data and memory are only stored locally.
- All secrets (API keys) are stored in the OS keychain GNOME Keyring/Secret Service. Secrets are never written in plaintext to config files.
- SQLite data is encrypted at rest via SQLCipher when a DB key is set.

## Requirements

- Rust 1.93 or newer
- 32GB+ of RAM
- Linux (Ubuntu recommended)
- Certain system libraries for Linux
- 24GB+ of VRAM (e.g. AMD 7900XTX or GTX 3090/4090/5090)

## Ollama Models Used

- ministral-3:14b (agent/router)
- embeddinggemma:latest (embedding)
- qllama/bge-reranker-v2-m3 (reranking)

## Supported Platforms/Devices

- (instant results) AMD Threadripper 2950X with 128GB DDR4 with AMD 7900XTX on Ubuntu 24.04.3

## Build

```bash
cargo build --release
```

## Run the UI

```bash
cargo run --release --bin butterfly-bot
```

Optional config import on launch:

```bash
cargo run --release --bin butterfly-bot -- --config config.json
```

Run CLI:

```bash
cargo run --bin butterfly-bot -- --cli
```

## Config

Use the Config tab in the app to configure:

- Provider credentials
- Tool enable/disable
- Reminders database path
- Memory settings

Config is stored in `./data/butterfly-bot.db` by default.

## SQLCipher (encrypted storage)

Butterfly Bot uses SQLCipher-backed SQLite when you provide a DB key. Set it via the CLI or environment:

```bash
cargo run --bin butterfly-bot -- db-key-set --key "your-strong-passphrase"
```

Or set the environment variable before running:

```bash
export BUTTERFLY_BOT_DB_KEY="your-strong-passphrase"
```

If no key is set, storage falls back to plaintext SQLite.

## Tools

### MCP Tool

Configure MCP servers in config.json under `tools.mcp.servers` (supports `type`: `sse` or `http`):

```json
{
    "tools": {
        "mcp": {
            "servers": [
                {
                    "name": "local",
                    "type": "sse",
                    "url": "http://127.0.0.1:3001/sse",
                    "headers": {
                        "Authorization": "Bearer my-token"
                    }
                }
            ]
        }
    }
}
```

HTTP (streamable) example:

```json
{
    "tools": {
        "mcp": {
            "servers": [
                {
                    "name": "github",
                    "type": "http",
                    "url": "https://api.githubcopilot.com/mcp/",
                    "headers": {
                        "Authorization": "Bearer YOUR_TOKEN"
                    }
                }
            ]
        }
    }
}
```

### Internet Search Tool

Configure the internet search tool in config.json under `tools.search_internet`:

```json
{
    "tools": {
        "search_internet": {
            "api_key": "YOUR_API_KEY",
            "provider": "openai",
            "model": "gpt-4o-mini-search-preview",
            "citations": true,
            "grok_web_search": true,
            "grok_x_search": true,
            "grok_timeout": 90,
            "network_allow": ["api.openai.com"],
            "default_deny": false
        }
    }
}
```

### Wakeup Tool

Create recurring agent tasks with `tools.wakeup`, control polling, and log runs to an audit file:

```json
{
    "tools": {
        "wakeup": {
            "poll_seconds": 60,
            "audit_log_path": "./data/wakeup_audit.log"
        }
    }
}
```

Wakeup runs are also streamed to the UI event feed as tool messages.

### HTTP Call Tool

Call external APIs with arbitrary HTTP requests and custom headers. Configure defaults under `tools.http_call`:

```json
{
    "tools": {
        "http_call": {
            "base_url": "https://api.example.com",
            "default_headers": {
                "Authorization": "Bearer YOUR_TOKEN"
            },
            "timeout_seconds": 60
        }
    }
}
```

### Todo Tool

Ordered todo list backed by SQLite:

```json
{
    "tools": {
        "todo": {
            "sqlite_path": "./data/butterfly-bot.db"
        }
    }
}
```

### Planning Tool

Structured plans with goals and steps:

```json
{
    "tools": {
        "planning": {
            "sqlite_path": "./data/butterfly-bot.db"
        }
    }
}
```

### Tasks Tool

Schedule one-off or recurring tasks with cancellation support:

```json
{
    "tools": {
        "tasks": {
            "poll_seconds": 60,
            "audit_log_path": "./data/tasks_audit.log",
            "sqlite_path": "./data/butterfly-bot.db"
        }
    }
}
```

## Library Usage (Minimal)

If you still want to embed Butterfly Bot, the Rust API is available:

```rust
use futures::StreamExt;
use butterfly_bot::client::ButterflyBot;

#[tokio::main]
async fn main() -> butterfly_bot::Result<()> {
    let agent = ButterflyBot::from_config_path("config.json").await?;
    let mut stream = agent.process_text_stream("user123", "Hello!", None);
    while let Some(chunk) = stream.next().await {
        print!("{}", chunk?);
    }
    Ok(())
}
```

## License

MIT
