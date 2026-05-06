use clap::Parser;
use serde::Serialize;

use crate::config::{DEFAULT_INDEXER_URL, DEFAULT_KUBO_API_URL};

fn clap_styles() -> clap::builder::Styles {
    use clap::builder::styling::{AnsiColor, Effects, Styles};

    Styles::styled()
        .header(AnsiColor::Green.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Cyan.on_default())
}

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "acuity-ipfs-pinner",
    next_line_help = true,
    color = clap::ColorChoice::Always,
    styles = clap_styles()
)]
pub struct Cli {
    #[arg(long, default_value = DEFAULT_INDEXER_URL)]
    pub indexer_url: String,

    #[arg(long, default_value = DEFAULT_KUBO_API_URL)]
    pub kubo_api_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_uses_default_urls() {
        let cli = Cli::parse_from(["acuity-ipfs-pinner"]);
        assert_eq!(cli.indexer_url, DEFAULT_INDEXER_URL);
        assert_eq!(cli.kubo_api_url, DEFAULT_KUBO_API_URL);
    }

    #[test]
    fn cli_accepts_overrides() {
        let cli = Cli::parse_from([
            "acuity-ipfs-pinner",
            "--indexer-url",
            "ws://example.test:8172",
            "--kubo-api-url",
            "http://example.test:5001",
        ]);
        assert_eq!(cli.indexer_url, "ws://example.test:8172");
        assert_eq!(cli.kubo_api_url, "http://example.test:5001");
    }
}
