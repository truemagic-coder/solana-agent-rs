## Butterfly Bot

`Butterfly Bot` is your personal AI assistant accessible via a native desktop app. It includes memory, tool integrations, a skill/heartbeat system, easy configuration, and streaming responses in a polished UI. The codebase still exposes a Rust library for building bots, but the primary focus is the app experience.

## Highlights

- Modern desktop UI (Dioxus) with streaming chat using local Ollama models or OpenAI compatible providers.
- Reminders and notifications both in chat and OS notifications.
- Optional long-term memory (temporal knowledge graph) using embedded local storage of SQLCipher and LanceDB.
- Skill and heartbeat Markdown files that define the assistant’s identity and ongoing guidance.
- Agent tool integrations with live UI events (tools are always on by default).
- Config and secrets managed from the config screen via JSON and stored in OS keychain for the best security.

## Architecture (Daemon + UI + Always-On Agent)

```
        ┌──────────────────────────────────────┐
        │           Desktop UI (Dioxus)        │
        │  - chat, config, notifications       │
        │  - streams tool + agent events       │
        └───────────────┬──────────────────────┘
                │ IPC / local client
                v
            ┌──────────────────────────────┐
            │      butterfly-botd          │
            │        (daemon)              │
            │  - always-on scheduler       │
            │  - tools + wakeups           │
            │  - memory + planning         │
            └──────────────┬───────────────┘
                   │
         ┌─────────────────┼─────────────────┐
         v                 v                 v
    ┌────────────────┐  ┌───────────────┐  ┌──────────────────┐
    │  Memory System │  │ Tooling Layer │  │  Model Provider  │
    │ (SQLCipher +   │  │ (MCP, HTTP,   │  │ (Ollama/OpenAI)  │
    │  LanceDB)      │  │ reminders,    │  │                  │
    │                │  │ tasks, etc.)  │  │                  │
    └────────────────┘  └───────────────┘  └──────────────────┘
```

### How this enables an always-on agent

- The agent is always-on only while the daemon is running. The daemon owns the scheduler, wakeups, and tool execution.
- If the UI shuts down and the daemon is also stopped, the agent will pause until the daemon is started again.
- Persistent memory and task queues live in the daemon’s storage, preserving context across restarts and long idle periods.

## Memory System (Diagram + Rationale)

```
                    ┌───────────────────────────────┐
                    │         Conversation          │
                    │  (raw turns + metadata)       │
                    └───────────────┬───────────────┘
                                    │
                                    v
                   ┌────────────────────────────────┐
                   │     Event + Signal Extractor   │
                   │ (facts, prefs, tasks, entities)│
                   └───────────────┬────────────────┘
                                   │
                     ┌─────────────┴─────────────┐
                     │                           │
                     v                           v
        ┌──────────────────────────┐   ┌──────────────────────────┐
        │  Temporal SQLCipher DB   │   │      LanceDB Vectors     │
        │  (structured memories)   │   │ (embeddings + rerank)    │
        └─────────────┬────────────┘   └─────────────┬────────────┘
                      │                              │
                      v                              v
        ┌──────────────────────────┐   ┌──────────────────────────┐
        │   Memory Summarizer      │   │  Semantic Recall + Rank  │
        │ (compression + pruning)  │   │ (query-time retrieval)   │
        └─────────────┬────────────┘   └─────────────┬────────────┘
                      └──────────────┬───────────────┘
                                     v
                        ┌────────────────────────┐
                        │   Context Assembler    │
                        │ (chat + tools + agent) │
                        └────────────────────────┘
```

### Temporal knowledge graph (what “temporal” means here)

Memory entries are stored as time-ordered events and entities in the SQLCipher database. Each fact, preference, reminder, and decision is recorded with timestamps and relationships, so recall can answer questions like “when did we decide this?” or “what changed since last week?” without relying on lossy summaries. This timeline-first structure is what makes the memory system a temporal knowledge graph rather than a static summary.

### Why this beats “just summarization” or QMD

- Summaries alone lose details. The system stores structured facts in SQLCipher and semantic traces in LanceDB so exact preferences, dates, and decisions remain queryable even after summarization.
- QMD-style recall can miss context. Dual storage (structured + vectors) plus reranking yields higher recall and fewer false positives.
- Temporal memory matters. The DB keeps time-ordered events so the assistant can answer “when did we decide X?” without relying on brittle summary phrasing.
- Safer pruning. Summarization is used for compression, not replacement, so older context is condensed while retaining anchors for precise retrieval.
- Faster, cheaper queries. Quick structured lookups handle facts and tasks; semantic search handles fuzzy recall, keeping prompts smaller and more relevant.

## Privacy & Security & Always On

- Run locally with Ollama to keep requests and model inference private on your machine.
- Designed for always-on use with unlimited token use (local inference) and customized wakeup and task intervals.
- Conversation data and memory are only stored locally.
- Config JSON is stored in the OS keychain.
- SQLite data is encrypted at rest via SQLCipher when a DB key is set.

## Ollama

### Requirements

- Rust 1.93 or newer
- 32GB+ of RAM 
- Linux (Ubuntu recommended)
- Certain system libraries for Linux
- 24GB+ of VRAM (e.g. AMD 7900XTX or GTX 3090/4090/5090)

### Models Used

- ministral-3:14b (assistant + summaries)
- embeddinggemma:latest (embedding)
- qllama/bge-reranker-v2-m3 (reranking)

### Model Notes
- Models are required to be pulled manually using `ollama pull` before `butterfly-bot` will work with them.
- Ollama models can be overriden and other models can be used rather than the default ones.
- Very beefy Macs like a max speced Mini or Studio could also run the Ollama setup (not tested)

### Test System

- AMD Threadripper 2950X with 128GB DDR4 with AMD 7900XTX on Ubuntu 24.04.3
- Provides instant results for Ollama chatting with memory

## OpenAI 

### Requirements

- Rust 1.93 or newer
- Certain system libraries for the host OS
- Mac or Linux or Windows (WSL)

### Model Recommendations

- No recommendations at this time as no testing of OpenAI has been done

## Build

```bash
cargo build --release
```

## Run

```bash
cargo run --release --bin butterfly-bot
```

## Config

Use the Config tab in the app to configure all settings via JSON. The config no longer includes an `agent` section — the assistant identity and behavior come from the skill Markdown.

Config is stored in the OS keychain for top security and safety.

### Skill & Heartbeat

- `skill_file` is a Markdown file (local path or URL) that defines the assistant’s identity, style, and rules.
- `heartbeat_file` is optional Markdown (local path or URL) that is appended to the system prompt for ongoing guidance.
- The heartbeat file is reloaded on every wakeup tick (using `tools.wakeup.poll_seconds`) so changes take effect without a restart.

### Minimal config example (Ollama defaults)

```json
{
    "openai": {
        "api_key": null,
        "model": "ministral-3:14b",
        "base_url": "http://localhost:11434/v1"
    },
    "skill_file": "./skill.md",
    "heartbeat_file": "./heartbeat.md",
    "memory": {
        "enabled": true,
        "sqlite_path": "./data/butterfly-bot.db",
        "lancedb_path": "./data/lancedb",
        "summary_model": "ministral-3:14b",
        "embedding_model": "embeddinggemma:latest",
        "rerank_model": "qllama/bge-reranker-v2-m3",
        "summary_threshold": null,
        "retention_days": null
    },
    "tools": {
        "settings": {
            "audit_log_path": "./data/tool_audit.log"
        }
    },
    "brains": {
        "settings": {
            "tick_seconds": 60
        }
    }
}
```

## SQLCipher (encrypted storage)

Butterfly Bot uses SQLCipher-backed SQLite when you provide a DB key. Set it via the CLI or environment:

```bash
cargo run --release --bin butterfly-bot -- db-key-set --key "your-strong-passphrase"
```

Or set the environment variable before running:

```bash
export BUTTERFLY_BOT_DB_KEY="your-strong-passphrase"
```

If no key is set, storage falls back to plaintext SQLite.

## Tools

### MCP Tool

The MCP tool supports both `SSE` or `HTTP` connections, custom headers, and multiple servers at once.

There are many high-quality MCP server providers like: 

* [Zapier](https://zapier.com/mcp) - 7,000+ app connections via MCP

* [VAPI.AI](https://vapi.ai) - Voice Agent Telephony 

* [GitHub](https://github.com) - Coding

Configure MCP servers under `tools.mcp.servers` (supports `type`: `sse` or `http`):

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

The Internet Search tool supports 3 different providers: `openai`, `grok`, and `perplexity`.

Configure the internet search tool under `tools.search_internet`:

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

The wakeup tool runs scheduled tasks on an interval.

Wakeup runs are also streamed to the UI event feed as tool messages.

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

### HTTP Call Tool

HTTP Call tool can call any public endpoint and private endpoint (if base url and authorization is provided).

Endpoints can be discovered by the agent or provided in the system/user prompts.

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

Ordered todo list backed by SQLite for the agent to created todo lists:

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

Structured plans with goals and steps for the agent to create plans:

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

Schedule one-off or recurring tasks with cancellation support for the agent to create tasks:

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

### Reminders Tool

The reminders tool is for users to create reminders for themselves or for the agent to create reminders for the user.

Create, list, complete, delete, and snooze reminders. Configure storage under `tools.reminders` (falls back to `memory.sqlite_path` if omitted):

```json
{
    "tools": {
        "reminders": {
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
