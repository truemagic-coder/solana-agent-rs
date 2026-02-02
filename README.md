## Butterfly Bot

Butterfly Bot is a desktop app for chatting with your personal AI assistant. It includes reminders, memory, tool integrations, and streaming responses in a polished UI. The codebase still exposes a Rust library for building bots, but the primary focus is the app experience.

## Highlights

- Modern desktop UI (Dioxus) with streaming chat using Ollama models
- Reminders and notifications both in chat and OS notifications
- Optional long-term memory using embedded local storage of SQLite and LanceDB
- Agent tool integrations with live UI events
- Config and secrets managed from the settings screen

## Privacy & Security

Privacy is a core principle for Butterfly Bot:

- Run locally with Ollama to keep requests and model inference on your machine.
- Conversation data and memory are stored locally by default.
- All secrets (API keys) are stored in the OS keychain GNOME Keyring/Secret Service. Secrets are never written in plaintext to config files.

## Requirements

- Rust 1.93 or newer
- Linux (Ubuntu recommended)
- Certain system libraries for Linux
- 24GB+ of VRAM (e.g. AMD 7900XTX or GTX 3090/4090/5090)

## Ollama Models Used

- ministral-3:14b (agent/router)
- embeddinggemma (embedding)
- qllama/bge-reranker-v2-m3 (reranking)

## Tested

- Tested on AMD Threadripper 2950X with 128GB DDR4 with AMD 7900XTX on Ubuntu 24.04.3

## Build

```bash
cargo build --release
```

## Run the UI

```bash
cargo run --release --bin butterfly-bot
```

Or run in dev:

```bash
cargo run --bin butterfly-bot -- --cli
```

When `base_url` is set, the API key is optional.

## Settings

Use the Settings tab in the app to configure:

- Provider credentials
- Tool enable/disable
- Reminders database path
- Memory settings

Config is stored in `./data/butterfly-bot.db` by default.

## CLI (Optional)

The CLI remains available for quick prompts or automation:

```bash
cargo run --release --bin butterfly-bot -- --cli
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
