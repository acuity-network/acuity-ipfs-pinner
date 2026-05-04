use acuity_ipfs_pinner::{Cli, Config, run};

#[tokio::main]
async fn main() {
    let cli = Cli::parse_from_env();
    let config = Config {
        indexer_url: cli.indexer_url,
        kubo_api_url: cli.kubo_api_url,
    };

    if let Err(error) = run(config).await {
        eprintln!("fatal error: {error}");
        std::process::exit(1);
    }
}
