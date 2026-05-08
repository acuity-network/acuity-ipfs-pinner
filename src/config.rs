pub const DEFAULT_INDEXER_URL: &str = "ws://127.0.0.1:8172";
pub const DEFAULT_KUBO_API_URL: &str = "http://127.0.0.1:5001";
pub const DEFAULT_ACK_PROTOCOL: &str = "/x/acuity/ack/1.0.0";

pub fn normalize_ack_protocol(protocol: &str) -> String {
    let trimmed = protocol.trim();

    if trimmed.starts_with("/x/") {
        trimmed.to_string()
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        format!("/x/{rest}")
    } else {
        format!("/x/{trimmed}")
    }
}

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

    #[test]
    fn normalize_ack_protocol_adds_x_namespace() {
        assert_eq!(normalize_ack_protocol("/acuity/ack/1.0.0"), "/x/acuity/ack/1.0.0");
        assert_eq!(normalize_ack_protocol("acuity/ack/1.0.0"), "/x/acuity/ack/1.0.0");
    }

    #[test]
    fn normalize_ack_protocol_preserves_existing_x_namespace() {
        assert_eq!(normalize_ack_protocol("/x/acuity/ack/1.0.0"), "/x/acuity/ack/1.0.0");
    }
}
