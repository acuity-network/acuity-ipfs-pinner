use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info};

pub const DEFAULT_INDEXER_URL: &str = "ws://127.0.0.1:8172";
pub const DEFAULT_KUBO_API_URL: &str = "http://127.0.0.1:5001";

#[derive(Debug)]
pub enum Error {
    WebSocket(tokio_tungstenite::tungstenite::Error),
    Http(reqwest::Error),
    Json(serde_json::Error),
    Protocol(String),
    InvalidIpfsHash(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebSocket(error) => write!(f, "websocket error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::InvalidIpfsHash(message) => write!(f, "invalid ipfs hash: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(value: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(value)
    }
}

impl From<reqwest::Error> for Error {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub indexer_url: String,
    pub kubo_api_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            indexer_url: DEFAULT_INDEXER_URL.to_string(),
            kubo_api_url: DEFAULT_KUBO_API_URL.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub result: T,
}

#[derive(Debug, Deserialize)]
pub struct MetadataResult {
    pub pallets: Vec<PalletMeta>,
}

#[derive(Debug, Deserialize)]
pub struct PalletMeta {
    pub index: u8,
    pub name: String,
    pub events: Vec<EventMeta>,
}

#[derive(Debug, Deserialize)]
pub struct EventMeta {
    pub index: u8,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionNotification {
    pub method: String,
    pub params: NotificationParams,
}

#[derive(Debug, Deserialize)]
pub struct NotificationParams {
    pub subscription: String,
    pub result: NotificationResult,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NotificationResult {
    Status {
        spans: Vec<Value>,
    },
    Event {
        key: Value,
        event: EventRef,
        #[serde(rename = "decodedEvent")]
        decoded_event: Option<DecodedEvent>,
    },
    Terminated {
        reason: String,
        message: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRef {
    pub block_number: u32,
    pub event_index: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedEvent {
    pub block_number: u32,
    pub event_index: u32,
    pub event: Value,
}

#[allow(async_fn_in_trait)]
pub trait PinClient {
    async fn pin(&self, cid: &str) -> Result<(), Error>;
}

#[derive(Clone)]
pub struct KuboClient {
    base_url: String,
    http: reqwest::Client,
}

impl KuboClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }
}

impl PinClient for KuboClient {
    async fn pin(&self, cid: &str) -> Result<(), Error> {
        let url = format!("{}/api/v0/pin/add", self.base_url);
        self.http
            .post(url)
            .query(&[("arg", cid)])
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

pub async fn run(config: Config) -> Result<(), Error> {
    let pinner = KuboClient::new(config.kubo_api_url.clone());

    loop {
        match run_once(&config, &pinner).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                error!(error = %error, "connection loop failed; retrying in 2s");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

async fn run_once(config: &Config, pinner: &impl PinClient) -> Result<(), Error> {
    let (mut ws, _) = connect_async(&config.indexer_url).await?;

    let (pallet_index, event_index) = lookup_publish_revision_variant(&mut ws).await?;
    let subscription_id = subscribe_to_variant(&mut ws, pallet_index, event_index).await?;
    info!(
        pallet_index,
        event_index, subscription_id, "subscribed to Content.PublishRevision"
    );

    while let Some(message) = ws.next().await {
        let message = message?;
        if let Message::Text(text) = message {
            match serde_json::from_str::<SubscriptionNotification>(&text) {
                Ok(notification) => {
                    if let Some(cid) = extract_publish_revision_cid(&notification)? {
                        info!(cid = %cid, "pinning content");
                        pinner.pin(&cid).await?;
                    }
                }
                Err(_) => {
                    // Ignore JSON-RPC responses and unrelated messages.
                }
            }
        }
    }

    Err(Error::Protocol("indexer websocket closed".into()))
}

pub async fn lookup_publish_revision_variant<S>(ws: &mut S) -> Result<(u8, u8), Error>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "acuity_getEventMetadata",
        "params": {}
    });
    ws.send(Message::Text(request.to_string().into())).await?;

    while let Some(message) = ws.next().await {
        let text = match message? {
            Message::Text(text) => text,
            _ => continue,
        };
        let response: JsonRpcResponse<MetadataResult> = match serde_json::from_str(&text) {
            Ok(response) => response,
            Err(_) => continue,
        };

        let pallet = response
            .result
            .pallets
            .into_iter()
            .find(|pallet| pallet.name == "Content")
            .ok_or_else(|| Error::Protocol("Content pallet not found in event metadata".into()))?;
        let event = pallet
            .events
            .into_iter()
            .find(|event| event.name == "PublishRevision")
            .ok_or_else(|| {
                Error::Protocol("Content.PublishRevision not found in event metadata".into())
            })?;
        return Ok((pallet.index, event.index));
    }

    Err(Error::Protocol(
        "websocket closed before event metadata response arrived".into(),
    ))
}

pub async fn subscribe_to_variant<S>(
    ws: &mut S,
    pallet_index: u8,
    event_index: u8,
) -> Result<String, Error>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "acuity_subscribeEvents",
        "params": {
            "key": {
                "type": "Variant",
                "value": [pallet_index, event_index]
            }
        }
    });
    ws.send(Message::Text(request.to_string().into())).await?;

    while let Some(message) = ws.next().await {
        let text = match message? {
            Message::Text(text) => text,
            _ => continue,
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(subscription_id) = value
            .get("result")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
        {
            return Ok(subscription_id);
        }
    }

    Err(Error::Protocol(
        "websocket closed before subscribe response arrived".into(),
    ))
}

pub fn extract_publish_revision_cid(
    notification: &SubscriptionNotification,
) -> Result<Option<String>, Error> {
    if notification.method != "acuity_subscription" {
        return Ok(None);
    }

    let NotificationResult::Event {
        decoded_event: Some(decoded_event),
        ..
    } = &notification.params.result
    else {
        return Ok(None);
    };

    let event = &decoded_event.event;
    if event.get("palletName").and_then(Value::as_str) != Some("Content")
        || event.get("eventName").and_then(Value::as_str) != Some("PublishRevision")
    {
        return Ok(None);
    }

    let ipfs_hash = event
        .get("fields")
        .and_then(|fields| fields.get("ipfs_hash"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            Error::Protocol("Content.PublishRevision missing fields.ipfs_hash".into())
        })?;

    Ok(Some(digest_hex_to_cid(ipfs_hash)?))
}

pub fn hex_to_bytes32(hex_value: &str) -> Result<[u8; 32], Error> {
    let raw = hex::decode(hex_value.trim_start_matches("0x")).map_err(|error| {
        Error::InvalidIpfsHash(format!("invalid hex value {hex_value}: {error}"))
    })?;
    raw.try_into()
        .map_err(|_| Error::InvalidIpfsHash(format!("expected 32 bytes for {hex_value}")))
}

pub fn digest_hex_to_cid(hex_value: &str) -> Result<String, Error> {
    let digest = hex_to_bytes32(hex_value)?;
    let mut multihash = Vec::with_capacity(34);
    multihash.push(0x12);
    multihash.push(0x20);
    multihash.extend_from_slice(&digest);
    Ok(bs58::encode(multihash).into_string())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_hex_to_cid_round_trip_known_digest() {
        let digest = "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let cid = digest_hex_to_cid(digest).unwrap();
        assert_eq!(cid, "QmNLfbof5rLekrACjeuLk9JmGZD2HDBHCU4z16iYKmx5SE");
    }

    #[test]
    fn extract_publish_revision_cid_from_notification() {
        let notification: SubscriptionNotification = serde_json::from_value(serde_json::json!({
            "method": "acuity_subscription",
            "params": {
                "subscription": "sub_1",
                "result": {
                    "type": "event",
                    "key": {"type": "Variant", "value": [4, 1]},
                    "event": {"blockNumber": 10, "eventIndex": 2},
                    "decodedEvent": {
                        "blockNumber": 10,
                        "eventIndex": 2,
                        "event": {
                            "specVersion": 1,
                            "palletName": "Content",
                            "eventName": "PublishRevision",
                            "fields": {
                                "ipfs_hash": "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
                            }
                        }
                    }
                }
            }
        }))
        .unwrap();

        let cid = extract_publish_revision_cid(&notification).unwrap();
        assert_eq!(
            cid.as_deref(),
            Some("QmNLfbof5rLekrACjeuLk9JmGZD2HDBHCU4z16iYKmx5SE")
        );
    }

    #[test]
    fn extract_publish_revision_cid_ignores_other_events() {
        let notification: SubscriptionNotification = serde_json::from_value(serde_json::json!({
            "method": "acuity_subscription",
            "params": {
                "subscription": "sub_1",
                "result": {
                    "type": "event",
                    "key": {"type": "Variant", "value": [4, 1]},
                    "event": {"blockNumber": 10, "eventIndex": 2},
                    "decodedEvent": {
                        "blockNumber": 10,
                        "eventIndex": 2,
                        "event": {
                            "specVersion": 1,
                            "palletName": "Balances",
                            "eventName": "Deposit",
                            "fields": {"amount": "10"}
                        }
                    }
                }
            }
        }))
        .unwrap();

        assert_eq!(extract_publish_revision_cid(&notification).unwrap(), None);
    }

    #[test]
    fn extract_publish_revision_cid_errors_when_hash_missing() {
        let notification: SubscriptionNotification = serde_json::from_value(serde_json::json!({
            "method": "acuity_subscription",
            "params": {
                "subscription": "sub_1",
                "result": {
                    "type": "event",
                    "key": {"type": "Variant", "value": [4, 1]},
                    "event": {"blockNumber": 10, "eventIndex": 2},
                    "decodedEvent": {
                        "blockNumber": 10,
                        "eventIndex": 2,
                        "event": {
                            "specVersion": 1,
                            "palletName": "Content",
                            "eventName": "PublishRevision",
                            "fields": {}
                        }
                    }
                }
            }
        }))
        .unwrap();

        assert!(matches!(
            extract_publish_revision_cid(&notification),
            Err(Error::Protocol(_))
        ));
    }
}
