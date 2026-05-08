use acuity_ipfs_pinner::{Cli, Config, normalize_ack_protocol, run};
use clap::Parser;
use tracing::error;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("acuity_ipfs_pinner=info")),
        )
        .init();

    let cli = Cli::parse();
    let config = Config {
        indexer_url: cli.indexer_url,
        kubo_api_url: cli.kubo_api_url,
        ack_protocol: normalize_ack_protocol(&cli.ack_protocol),
    };

    if let Err(error) = run(config).await {
        error!(error = %error, "fatal error");
        std::process::exit(1);
    }
}
