use serde::Serialize;

use crate::config::{DEFAULT_INDEXER_URL, DEFAULT_KUBO_API_URL};

#[derive(Debug, Serialize)]
pub struct Cli {
    pub indexer_url: String,
    pub kubo_api_url: String,
}

impl Cli {
    pub fn parse_from_env() -> Self {
        let mut cli = Self {
            indexer_url: DEFAULT_INDEXER_URL.to_string(),
            kubo_api_url: DEFAULT_KUBO_API_URL.to_string(),
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--indexer-url" => {
                    cli.indexer_url = args
                        .next()
                        .unwrap_or_else(|| panic!("missing value for --indexer-url"));
                }
                "--kubo-api-url" => {
                    cli.kubo_api_url = args
                        .next()
                        .unwrap_or_else(|| panic!("missing value for --kubo-api-url"));
                }
                "-h" | "--help" => {
                    print_help_and_exit();
                }
                other => panic!("unknown argument: {other}"),
            }
        }

        cli
    }
}

fn print_help_and_exit() -> ! {
    println!("acuity-ipfs-pinner");
    println!("  --indexer-url <URL>   acuity-index websocket URL [default: {DEFAULT_INDEXER_URL}]");
    println!("  --kubo-api-url <URL>  Kubo HTTP API URL [default: {DEFAULT_KUBO_API_URL}]");
    std::process::exit(0);
}
