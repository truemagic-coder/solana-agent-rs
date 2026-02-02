use butterfly_bot::daemon;
use butterfly_bot::error::Result;
use butterfly_bot::sqlcipher;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "butterfly-botd")]
#[command(about = "ButterFly Bot local daemon")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 7878)]
    port: u16,

    #[arg(long, default_value = "./data/butterfly-bot.db")]
    db: String,

    #[arg(long, env = "BUTTERFLY_BOT_TOKEN", default_value = "")]
    token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,butterfly_bot=info,lance=warn,lancedb=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    sqlcipher::configure_sqlcipher_logging();
    let cli = Cli::parse();

    daemon::run(&cli.host, cli.port, &cli.db, &cli.token).await
}
