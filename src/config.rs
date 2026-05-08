pub const DEFAULT_INDEXER_URL: &str = "ws://127.0.0.1:8172";
pub const DEFAULT_KUBO_API_URL: &str = "http://127.0.0.1:5001";
pub const DEFAULT_ACK_PROTOCOL: &str = "/acuity/ack/1.0.0";

#[derive(Debug, Clone)]
pub struct Config {
    pub indexer_url: String,
    pub kubo_api_url: String,
    pub ack_protocol: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            indexer_url: DEFAULT_INDEXER_URL.to_string(),
            kubo_api_url: DEFAULT_KUBO_API_URL.to_string(),
            ack_protocol: DEFAULT_ACK_PROTOCOL.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_expected_urls() {
        let config = Config::default();
        assert_eq!(config.indexer_url, DEFAULT_INDEXER_URL);
        assert_eq!(config.kubo_api_url, DEFAULT_KUBO_API_URL);
        assert_eq!(config.ack_protocol, DEFAULT_ACK_PROTOCOL);
    }
}
