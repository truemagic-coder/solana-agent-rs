## Butterfly Bot

Butterfly Bot is a desktop app for chatting with your personal AI assistant. It includes reminders, memory, tool integrations, and streaming responses in a sleek UI. The codebase still exposes a Rust library for building bots, but the primary focus is the app experience.

## Highlights

- Modern desktop UI (Dioxus) with streaming chat
- Reminders and notifications
- Optional memory with local storage
- Tool integrations with live UI events
- OpenAI-compatible providers (OpenAI, Ollama, etc.)
- Config and secrets managed from the app Settings

## Requirements

- Rust 1.93 or newer
- An OpenAI API key, or an OpenAI-compatible endpoint (e.g., Ollama)

## Build

```bash
cargo build --release
```

## Run the UI

```bash
cargo run --release
```

Or run in dev:

```bash
cargo run
```

The UI starts a local daemon automatically unless you set `BUTTERFLY_BOT_DISABLE_DAEMON=1`.

## Settings

Use the Settings tab in the app to configure:

- Provider credentials
- Tool enable/disable
- Reminders database path
- Memory settings

Config is stored in `./data/butterfly-bot.db` by default.
```

## CLI (Optional)

The CLI remains available for quick prompts or automation:

```bash
./target/release/butterfly-bot --cli
```

## Library Usage

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
    }
  });

  let options = ProcessOptions {
    prompt: None,
    images: vec![],
    output_format: OutputFormat::Text,
    image_detail: "auto".to_string(),
    json_schema: Some(schema),
    router: None,
  };

  let result = agent
    .process(
      "user123",
      UserInput::Text("Summarize AI news".to_string()),
      options,
    )
    .await?;

  if let ProcessResult::Structured(value) = result {
    println!("{}", value);
  }

  Ok(())
}
```

  ## Configuration in Code (Preferred)

  Most apps build config in code using environment variables. The file-based config is intended for the CLI.

  ```rust
  use std::env;

  use butterfly_bot::config::{AgentConfig, Config, OpenAiConfig};
  use butterfly_bot::client::ButterflyBot;

  #[tokio::main]
  async fn main() -> butterfly_bot::Result<()> {
    let config = Config {
      openai: Some(OpenAiConfig {
        api_key: env::var("OPENAI_API_KEY").ok(),
        model: Some("gpt-5.2".to_string()),
        base_url: None,
      }),
      agents: vec![AgentConfig {
        name: "default_agent".to_string(),
        instructions: "You are a helpful AI assistant.".to_string(),
        specialization: "general".to_string(),
        description: None,
        capture_name: None,
        capture_schema: None,
      }],
      business: None,
      memory: None,
      guardrails: None,
      tools: None,
    };

    let agent = ButterflyBot::from_config(config).await?;
    let mut stream = agent.process_text_stream("user123", "Hello!", None);
    while let Some(chunk) = stream.next().await {
      let text = chunk?;
      print!("{}", text);
    }
    Ok(())
  }
  ```

## Configuration Reference

Top-level schema:

```json
{
  "openai": {
    "api_key": "...",
    "model": "gpt-5.2",
    "base_url": "https://api.openai.com/v1"
  },
  "agents": [
    {
      "name": "...",
      "instructions": "...",
      "specialization": "...",
      "description": "...",
      "capture_name": "...",
      "capture_schema": {}
    }
  ],
  "business": {
    "mission": "...",
    "voice": "...",
    "values": [
      { "name": "...", "description": "..." }
    ],
    "goals": ["..."]
  },
  "memory": {
    "enabled": true,
    "sqlite_path": "./data/butterfly-bot.db",
    "lancedb_path": "./data/lancedb",
    "embedding_model": "ollama:qwen3-embedding",
    "rerank_model": "ollama:qwen3-reranker"
  },
  "guardrails": {
    "input": [
      { "class": "butterfly_bot.guardrails.pii.PII", "config": { "replacement": "[REDACTED]" } }
    ],
    "output": [
      { "class": "butterfly_bot.guardrails.pii.PII" }
    ]
  }
}
```

Notes:

- `openai.api_key` is optional when using an OpenAI-compatible `base_url` (e.g., Ollama).

- `openai.model` defaults to `gpt-5.2` if omitted.
- `agents` must contain at least one agent.
- `capture_*` fields are accepted and stored but not yet used by the Rust runtime.
- If `memory` is omitted, history is kept in-memory only (lost on restart).
- Use `butterfly-bot config export --path <file>` to export a redacted config.

## Local Memory (SQLite + LanceDB)

Local persistent memory uses embedded SQLite for transcripts and LanceDB for vectors. Add a `memory` block to your config:

```json
{
  "memory": {
    "enabled": true,
    "sqlite_path": "./data/butterfly-bot.db",
    "lancedb_path": "./data/lancedb"
  }
}
```

## Routing Behavior

The router selects an agent by matching query text against agent names and specialization keywords. If only one agent exists, it is always selected.

You can override routing per request by passing a custom router in `ProcessOptions.router` (implement the `RoutingService` trait).

## Memory Behavior

The default setup stores conversation history in memory only (process lifetime). You can clear history per user with `delete_user_history`.

## Memory Provider Interface

The Rust memory interface mirrors the Python provider shape for history and captures:

- `store(user_id, messages)`
- `retrieve(user_id)`
- `delete(user_id)`
- `find(collection, query, sort, limit, skip)`
- `count_documents(collection, query)`
- `save_capture(user_id, capture_name, agent_name, data, schema)`

With local memory enabled, messages are stored in SQLite and captures are stored in the local `captures` table.

## Guardrails

Guardrails can be configured in the Rust config using class names compatible with the Python naming convention. Currently supported:

- `butterfly_bot.guardrails.pii.PII` (basic email/phone scrubbing)

```json
{
  "guardrails": {
    "input": [
      { "class": "butterfly_bot.guardrails.pii.PII", "config": { "replacement": "[REDACTED]" } }
    ],
    "output": [
      { "class": "butterfly_bot.guardrails.pii.PII" }
    ]
  }
}
```

## Tooling

You can register tools by implementing the `Tool` trait and calling `register_tool` on `ButterflyBot`.

Tool calls are executed automatically when the model requests them. Tool results are fed back into the model until a final response is produced.

Tool safety is driven by config settings:

- `tools.settings.permissions.default_deny` (bool)
- `tools.settings.permissions.network_allow` (list of domains)
- `tools.settings.audit_log_path` (path, defaults to `./data/tool_audit.log`)

Tool-specific overrides can be set in `tools.<tool_name>.permissions.network_allow`.

Brain settings:

- `brains.settings.tick_seconds` (u64, default `60`)

## License

MIT
