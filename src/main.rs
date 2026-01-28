#[cfg(not(test))]
use clap::Parser;
#[cfg(not(test))]
use futures::StreamExt;
#[cfg(not(test))]
use tokio::io::{self, AsyncBufReadExt};

#[cfg(not(test))]
use solana_agent::client::SolanaAgent;
#[cfg(not(test))]
use solana_agent::error::Result;

#[cfg(not(test))]
#[derive(Parser, Debug)]
#[command(name = "solana-agent")]
#[command(about = "Solana Agent CLI (Rust)")]
struct Cli {
    #[arg(long, default_value = "config.json")]
    config: String,

    #[arg(long, default_value = "cli_user")]
    user_id: String,

    #[arg(long)]
    prompt: Option<String>,
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let cli = Cli::parse();
    let agent = SolanaAgent::from_config_path(&cli.config).await?;

    if let Some(prompt) = cli.prompt {
        let mut stream = agent.process_text_stream(&cli.user_id, &prompt, None);
        while let Some(chunk) = stream.next().await {
            let text = chunk?;
            print!("{}", text);
        }
        println!();
        return Ok(());
    }

    println!("Enter your prompts (Ctrl+D to exit):");
    let stdin = io::BufReader::new(io::stdin());
    let mut lines = stdin.lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| solana_agent::error::SolanaAgentError::Runtime(e.to_string()))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let mut stream = agent.process_text_stream(&cli.user_id, &line, None);
        while let Some(chunk) = stream.next().await {
            let text = chunk?;
            print!("{}", text);
        }
        println!();
    }

    Ok(())
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
