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
