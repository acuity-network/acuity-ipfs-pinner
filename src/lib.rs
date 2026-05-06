use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, fmt, net::IpAddr, path::PathBuf, process::Stdio};
use tokio::{
    process::{Child, Command},
    signal,
    time::{Duration, Instant},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

pub const DEFAULT_INDEXER_URL: &str = "ws://127.0.0.1:8172";
pub const DEFAULT_KUBO_API_URL: &str = "http://127.0.0.1:5001";

#[derive(Debug)]
pub enum Error {
    WebSocket(tokio_tungstenite::tungstenite::Error),
    Http(reqwest::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    ProstDecode(prost::DecodeError),
    Protocol(String),
    InvalidIpfsHash(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebSocket(error) => write!(f, "websocket error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::ProstDecode(error) => write!(f, "protobuf decode error: {error}"),
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

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<prost::DecodeError> for Error {
    fn from(value: prost::DecodeError) -> Self {
        Self::ProstDecode(value)
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

#[derive(Debug, Deserialize)]
pub struct KuboIdResponse {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "PublicKey")]
    pub public_key: Option<String>,
    #[serde(rename = "Addresses", default)]
    pub addresses: Vec<String>,
    #[serde(rename = "AgentVersion")]
    pub agent_version: Option<String>,
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: Option<String>,
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

impl KuboClient {
    pub async fn id(&self) -> Result<KuboIdResponse, Error> {
        let url = format!("{}/api/v0/id", self.base_url);
        let timeout = std::time::Duration::from_secs(10);

        let response = self.http.post(&url).timeout(timeout).send().await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            warn!(%url, %status, body = %body, "kubo id request failed");
            return Err(Error::Protocol(format!(
                "kubo id failed with status {status}: {body}"
            )));
        }

        let id = serde_json::from_str::<KuboIdResponse>(&body)?;
        info!(
            peer_id = %id.id,
            agent_version = id.agent_version.as_deref().unwrap_or("<unknown>"),
            protocol_version = id.protocol_version.as_deref().unwrap_or("<unknown>"),
            address_count = id.addresses.len(),
            "connected to kubo node"
        );
        Ok(id)
    }

    pub async fn pin(&self, cid: &str) -> Result<(), Error> {
        let url = format!("{}/api/v0/pin/add", self.base_url);
        let timeout = std::time::Duration::from_secs(30);
        info!(cid = %cid, %url, timeout_secs = timeout.as_secs(), "starting kubo pin/add request");

        let response = self
            .http
            .post(&url)
            .query(&[("arg", cid)])
            .timeout(timeout)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            warn!(cid = %cid, %url, %status, body = %body, "kubo pin/add failed");
            return Err(Error::Protocol(format!(
                "kubo pin/add failed with status {status}: {body}"
            )));
        }

        info!(cid = %cid, %url, %status, body = %body, "kubo pin/add completed");
        Ok(())
    }

    pub async fn cat(&self, cid: &str) -> Result<Vec<u8>, Error> {
        let url = format!("{}/api/v0/cat", self.base_url);
        let timeout = std::time::Duration::from_secs(30);
        info!(cid = %cid, %url, timeout_secs = timeout.as_secs(), "starting kubo cat request");

        let response = self
            .http
            .post(&url)
            .query(&[("arg", cid)])
            .timeout(timeout)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            warn!(cid = %cid, %url, %status, body = %body, "kubo cat failed");
            return Err(Error::Protocol(format!(
                "kubo cat failed with status {status}: {body}"
            )));
        }

        let bytes = response.bytes().await?.to_vec();
        info!(cid = %cid, %url, %status, byte_len = bytes.len(), "kubo cat completed");
        Ok(bytes)
    }
}

fn resolve_kubo_repo_dir() -> Result<PathBuf, Error> {
    match home::home_dir() {
        Some(mut path) => {
            path.push(".local/share/acuity-ipfs-pinner");
            path.push("ipfs-repo");
            Ok(path)
        }
        None => Err(Error::Protocol("no home directory".into())),
    }
}

async fn ensure_kubo_repo_initialized(repo_dir: &std::path::Path) -> Result<(), Error> {
    info!(repo_dir = %repo_dir.display(), "initializing kubo repo");

    if let Some(parent_dir) = repo_dir.parent() {
        std::fs::create_dir_all(parent_dir)?;
    }

    let output = Command::new("ipfs")
        .arg("init")
        .arg("--repo-dir")
        .arg(repo_dir)
        .output()
        .await?;

    match output.status.code() {
        Some(0) => {
            info!(repo_dir = %repo_dir.display(), "created new kubo repo");
            Ok(())
        }
        Some(1) => {
            info!(repo_dir = %repo_dir.display(), "kubo repo already existed");
            Ok(())
        }
        _ => Err(Error::Protocol(format!(
            "ipfs init failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))),
    }
}

async fn configure_kubo_swarm_addresses(repo_dir: &std::path::Path) -> Result<(), Error> {
    const SWARM_ADDRESSES: &str = r#"[
  "/ip4/0.0.0.0/tcp/4001",
  "/ip4/0.0.0.0/tcp/4002/ws",
  "/ip6/::/tcp/4001",
  "/ip6/::/tcp/4002/ws",
  "/ip4/0.0.0.0/udp/4001/webrtc-direct",
  "/ip4/0.0.0.0/udp/4001/quic-v1",
  "/ip4/0.0.0.0/udp/4001/quic-v1/webtransport",
  "/ip6/::/udp/4001/webrtc-direct",
  "/ip6/::/udp/4001/quic-v1",
  "/ip6/::/udp/4001/quic-v1/webtransport"
]"#;

    info!(repo_dir = %repo_dir.display(), "configuring kubo swarm addresses");
    let output = Command::new("ipfs")
        .arg("config")
        .arg("--json")
        .arg("Addresses.Swarm")
        .arg(SWARM_ADDRESSES)
        .arg("--repo-dir")
        .arg(repo_dir)
        .output()
        .await?;

    if output.status.success() {
        info!(repo_dir = %repo_dir.display(), "configured kubo swarm addresses");
        Ok(())
    } else {
        Err(Error::Protocol(format!(
            "ipfs config Addresses.Swarm failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.segments()[0] & 0xffc0 == 0xfec0
        }
    }
}

fn normalize_ws_multiaddr(address: &str, peer_id: &str) -> Option<String> {
    let protocol = if address.starts_with("/ip4/") {
        "/ip4/"
    } else if address.starts_with("/ip6/") {
        "/ip6/"
    } else {
        return None;
    };

    let Some(ws_index) = address.find("/ws") else {
        return None;
    };

    let ws_suffix = &address[ws_index..];
    if ws_suffix != "/ws" && !ws_suffix.starts_with("/ws/") {
        return None;
    }

    let after_protocol = &address[protocol.len()..];
    let ip_end = after_protocol.find('/')?;
    after_protocol[..ip_end].parse::<IpAddr>().ok()?;
    let base = if let Some(p2p_index) = address.find("/p2p/") {
        &address[..p2p_index]
    } else {
        address
    };

    Some(format!("{}/p2p/{}", base.trim_end_matches('/'), peer_id))
}

fn log_kubo_ws_multiaddrs(id: &KuboIdResponse) {
    let mut local = BTreeSet::new();
    let mut external = BTreeSet::new();

    for address in &id.addresses {
        let Some(normalized) = normalize_ws_multiaddr(address, &id.id) else {
            continue;
        };

        let ip = if let Some(rest) = normalized.strip_prefix("/ip4/") {
            rest.split('/').next().and_then(|value| value.parse().ok())
        } else if let Some(rest) = normalized.strip_prefix("/ip6/") {
            rest.split('/').next().and_then(|value| value.parse().ok())
        } else {
            None
        };

        let Some(ip) = ip else {
            continue;
        };

        if is_local_ip(ip) {
            local.insert(normalized);
        } else {
            external.insert(normalized);
        }
    }

    for address in local {
        info!("{address}");
    }

    for address in external {
        info!("{address}");
    }
}

async fn wait_for_kubo_api(kubo: &KuboClient, daemon: &mut Child) -> Result<KuboIdResponse, Error> {
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        if let Some(status) = daemon.try_wait()? {
            return Err(Error::Protocol(format!(
                "ipfs daemon exited before becoming available: {status}"
            )));
        }

        match kubo.id().await {
            Ok(id) => return Ok(id),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(Error::Protocol(format!(
                        "timed out waiting for kubo api: {error}"
                    )));
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn start_kubo_daemon(kubo: &KuboClient) -> Result<Option<Child>, Error> {
    let repo_dir = resolve_kubo_repo_dir()?;

    if let Ok(id) = kubo.id().await {
        info!(repo_dir = %repo_dir.display(), "kubo api already available");
        log_kubo_ws_multiaddrs(&id);
        return Ok(None);
    }

    ensure_kubo_repo_initialized(&repo_dir).await?;
    configure_kubo_swarm_addresses(&repo_dir).await?;

    info!(repo_dir = %repo_dir.display(), "starting ipfs daemon");
    let mut daemon = Command::new("ipfs")
        .arg("daemon")
        .arg("--repo-dir")
        .arg(&repo_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let id = wait_for_kubo_api(kubo, &mut daemon).await?;
    info!(repo_dir = %repo_dir.display(), "ipfs daemon is available");
    log_kubo_ws_multiaddrs(&id);
    Ok(Some(daemon))
}

async fn stop_kubo_daemon(daemon: &mut Child) -> Result<(), Error> {
    if let Some(status) = daemon.try_wait()? {
        info!(status = %status, "ipfs daemon already exited");
        return Ok(());
    }

    let Some(pid) = daemon.id() else {
        return Err(Error::Protocol("ipfs daemon pid unavailable".into()));
    };

    info!(pid, "sending SIGINT to ipfs daemon");
    let status = Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .await?;

    if !status.success() {
        return Err(Error::Protocol(format!(
            "failed to send SIGINT to ipfs daemon: {status}"
        )));
    }

    match tokio::time::timeout(Duration::from_secs(30), daemon.wait()).await {
        Ok(status) => {
            let status = status?;
            info!(status = %status, "ipfs daemon stopped");
            Ok(())
        }
        Err(_) => {
            warn!(pid, "timed out waiting for ipfs daemon to stop; killing");
            daemon.kill().await?;
            let status = daemon.wait().await?;
            warn!(status = %status, "ipfs daemon killed after shutdown timeout");
            Ok(())
        }
    }
}

pub async fn run(config: Config) -> Result<(), Error> {
    let pinner = KuboClient::new(config.kubo_api_url.clone());
    let mut kubo_daemon = start_kubo_daemon(&pinner).await?;

    loop {
        match run_once(&config, pinner.clone()).await {
            Ok(()) => break,
            Err(error) => {
                error!(error = %error, "connection loop failed; retrying in 2s");
                tokio::select! {
                    _ = signal::ctrl_c() => {
                        info!(indexer_url = %config.indexer_url, kubo_api_url = %config.kubo_api_url, "received Ctrl+C while waiting to reconnect; shutting down network connections");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
        }
    }

    info!(indexer_url = %config.indexer_url, kubo_api_url = %config.kubo_api_url, kubo_daemon_managed = kubo_daemon.is_some(), "shutdown sequence complete for application network connections");

    if let Some(mut daemon) = kubo_daemon.take() {
        stop_kubo_daemon(&mut daemon).await?;
    }

    Ok(())
}

async fn close_indexer_connection<S>(
    ws: &mut S,
    indexer_url: &str,
    subscription_id: &str,
) -> Result<(), Error>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    info!(
        indexer_url,
        subscription_id, "sending websocket close to indexer"
    );
    ws.send(Message::Close(None)).await?;

    match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
        Ok(Some(Ok(Message::Close(frame)))) => {
            info!(indexer_url, subscription_id, close_frame = ?frame, "indexer acknowledged websocket close");
        }
        Ok(Some(Ok(message))) => {
            info!(indexer_url, subscription_id, message = ?message, "received final websocket message during shutdown");
        }
        Ok(Some(Err(error))) => {
            warn!(indexer_url, subscription_id, error = %error, "error while closing indexer websocket");
        }
        Ok(None) => {
            info!(indexer_url, subscription_id, "indexer websocket closed");
        }
        Err(_) => {
            warn!(
                indexer_url,
                subscription_id, "timed out waiting for indexer websocket to close"
            );
        }
    }

    Ok(())
}

async fn run_once(config: &Config, pinner: KuboClient) -> Result<(), Error> {
    let (mut ws, _) = connect_async(&config.indexer_url).await?;
    info!(indexer_url = %config.indexer_url, kubo_api_url = %config.kubo_api_url, "network connections established");

    let (pallet_index, event_index) = lookup_publish_revision_variant(&mut ws).await?;
    let subscription_id = subscribe_to_variant(&mut ws, pallet_index, event_index).await?;
    info!(
        indexer_url = %config.indexer_url,
        kubo_api_url = %config.kubo_api_url,
        pallet_index,
        event_index,
        subscription_id,
        "subscribed to Content.PublishRevision"
    );

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!(indexer_url = %config.indexer_url, kubo_api_url = %config.kubo_api_url, subscription_id, "received Ctrl+C; gracefully closing network connections");
                close_indexer_connection(&mut ws, &config.indexer_url, &subscription_id).await?;
                return Ok(());
            }
            message = ws.next() => {
                let Some(message) = message else {
                    break;
                };
                let message = message?;
                if let Message::Text(text) = message {
                    match serde_json::from_str::<SubscriptionNotification>(&text) {
                        Ok(notification) => {
                            info!(
                                subscription = %notification.params.subscription,
                                method = %notification.method,
                                payload = %text,
                                "received subscription notification"
                            );

                            match extract_publish_revision(&notification) {
                                Ok(Some(revision)) => {
                                    info!(
                                        item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                        owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                        revision_id = revision.revision_id.unwrap_or_default(),
                                        cid = %revision.cid,
                                        "queueing background pin"
                                    );

                                    let pin_client = pinner.clone();
                                    tokio::spawn(async move {
                                        info!(
                                            item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                            owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                            revision_id = revision.revision_id.unwrap_or_default(),
                                            cid = %revision.cid,
                                            "pinning content"
                                        );

                                        match pin_client.pin(&revision.cid).await {
                                            Ok(()) => {
                                                info!(
                                                    item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                    owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                    revision_id = revision.revision_id.unwrap_or_default(),
                                                    cid = %revision.cid,
                                                    "pinned content"
                                                );

                                                match pin_client.cat(&revision.cid).await {
                                                    Ok(item_bytes) => match extract_image_cids_from_item_bytes(&item_bytes) {
                                                        Ok(image_cids) => {
                                                            if image_cids.is_empty() {
                                                                info!(
                                                                    item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                                    owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                                    revision_id = revision.revision_id.unwrap_or_default(),
                                                                    cid = %revision.cid,
                                                                    "no image mixin CIDs found in pinned content"
                                                                );
                                                            } else {
                                                                info!(
                                                                    item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                                    owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                                    revision_id = revision.revision_id.unwrap_or_default(),
                                                                    cid = %revision.cid,
                                                                    image_cid_count = image_cids.len(),
                                                                    image_cids = ?image_cids,
                                                                    "extracted image mixin CIDs from pinned content"
                                                                );

                                                                for image_cid in image_cids {
                                                                    info!(
                                                                        item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                                        owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                                        revision_id = revision.revision_id.unwrap_or_default(),
                                                                        parent_cid = %revision.cid,
                                                                        cid = %image_cid,
                                                                        "pinning image mixin content"
                                                                    );

                                                                    match pin_client.pin(&image_cid).await {
                                                                        Ok(()) => {
                                                                            info!(
                                                                                item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                                                owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                                                revision_id = revision.revision_id.unwrap_or_default(),
                                                                                parent_cid = %revision.cid,
                                                                                cid = %image_cid,
                                                                                "pinned image mixin content"
                                                                            );
                                                                        }
                                                                        Err(error) => {
                                                                            warn!(
                                                                                item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                                                owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                                                revision_id = revision.revision_id.unwrap_or_default(),
                                                                                parent_cid = %revision.cid,
                                                                                cid = %image_cid,
                                                                                error = %error,
                                                                                "image mixin pin failed"
                                                                            );
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        Err(error) => {
                                                            warn!(
                                                                item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                                owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                                revision_id = revision.revision_id.unwrap_or_default(),
                                                                cid = %revision.cid,
                                                                error = %error,
                                                                "failed to decode pinned content protobuf for image extraction"
                                                            );
                                                        }
                                                    },
                                                    Err(error) => {
                                                        warn!(
                                                            item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                            owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                            revision_id = revision.revision_id.unwrap_or_default(),
                                                            cid = %revision.cid,
                                                            error = %error,
                                                            "failed to read pinned content from kubo for image extraction"
                                                        );
                                                    }
                                                }
                                            }
                                            Err(error) => {
                                                warn!(
                                                    item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
                                                    owner = revision.owner.as_deref().unwrap_or("<missing>"),
                                                    revision_id = revision.revision_id.unwrap_or_default(),
                                                    cid = %revision.cid,
                                                    error = %error,
                                                    "background pin failed"
                                                );
                                            }
                                        }
                                    });
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    warn!(error = %error, payload = %text, "ignoring malformed publish revision event");
                                }
                            }
                        }
                        Err(_) => {
                            // Ignore JSON-RPC responses and unrelated messages.
                        }
                    }
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

pub const IMAGE_MIXIN_ID: u32 = 0x045e_ee8c;

#[derive(Clone, PartialEq, prost::Message)]
pub struct ItemMessage {
    #[prost(message, repeated, tag = "1")]
    pub mixin_payload: Vec<MixinPayloadMessage>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct MixinPayloadMessage {
    #[prost(fixed32, tag = "1")]
    pub mixin_id: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ImageMixinMessage {
    #[prost(string, tag = "1")]
    pub filename: String,
    #[prost(uint64, tag = "2")]
    pub filesize: u64,
    #[prost(bytes = "vec", tag = "3")]
    pub ipfs_hash: Vec<u8>,
    #[prost(uint32, tag = "4")]
    pub width: u32,
    #[prost(uint32, tag = "5")]
    pub height: u32,
    #[prost(message, repeated, tag = "6")]
    pub mipmap_level: Vec<MipmapLevelMessage>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct MipmapLevelMessage {
    #[prost(uint64, tag = "1")]
    pub filesize: u64,
    #[prost(bytes = "vec", tag = "2")]
    pub ipfs_hash: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRevision {
    pub item_id: Option<String>,
    pub owner: Option<String>,
    pub revision_id: Option<u32>,
    pub cid: String,
}

pub fn extract_publish_revision(
    notification: &SubscriptionNotification,
) -> Result<Option<PublishRevision>, Error> {
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

    let fields = event
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Protocol("Content.PublishRevision missing fields".into()))?;

    info!(fields = %serde_json::Value::Object(fields.clone()), "decoded Content.PublishRevision fields");

    let ipfs_hash = fields
        .get("ipfs_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            Error::Protocol("Content.PublishRevision missing fields.ipfs_hash".into())
        })?;

    Ok(Some(PublishRevision {
        item_id: fields
            .get("item_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        owner: fields
            .get("owner")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        revision_id: fields.get("revision_id").and_then(parse_u32_value),
        cid: digest_hex_to_cid(ipfs_hash)?,
    }))
}

fn parse_u32_value(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::String(string) => string.parse::<u32>().ok(),
        _ => None,
    }
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
    Ok(multihash_bytes_to_cid(&digest_to_multihash_bytes(&digest)))
}

fn digest_to_multihash_bytes(digest: &[u8; 32]) -> Vec<u8> {
    let mut multihash = Vec::with_capacity(34);
    multihash.push(0x12);
    multihash.push(0x20);
    multihash.extend_from_slice(digest);
    multihash
}

fn multihash_bytes_to_cid(multihash: &[u8]) -> String {
    bs58::encode(multihash).into_string()
}

fn extract_image_cids_from_item_bytes(bytes: &[u8]) -> Result<Vec<String>, Error> {
    let item = ItemMessage::decode(bytes)?;
    let mut cids = BTreeSet::new();

    for mixin in item.mixin_payload {
        if mixin.mixin_id != IMAGE_MIXIN_ID {
            continue;
        }

        let image = ImageMixinMessage::decode(mixin.payload.as_slice())?;
        if !image.ipfs_hash.is_empty() {
            cids.insert(multihash_bytes_to_cid(&image.ipfs_hash));
        }
        for mipmap in image.mipmap_level {
            if !mipmap.ipfs_hash.is_empty() {
                cids.insert(multihash_bytes_to_cid(&mipmap.ipfs_hash));
            }
        }
    }

    Ok(cids.into_iter().collect())
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
    fn extract_publish_revision_from_notification() {
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
                                "item_id": "0xdc58d9c8f1ed90f432d5b49bdc82ae3f617fd513b3995814d1e37d541bc72161",
                                "owner": "5ERcQ879d4HofcDgzTxaZXo98JNZvjKWQcisM8thyAkj8ztQ",
                                "revision_id": 1,
                                "ipfs_hash": "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
                            }
                        }
                    }
                }
            }
        }))
        .unwrap();

        let revision = extract_publish_revision(&notification).unwrap().unwrap();
        assert_eq!(
            revision.item_id.as_deref(),
            Some("0xdc58d9c8f1ed90f432d5b49bdc82ae3f617fd513b3995814d1e37d541bc72161")
        );
        assert_eq!(
            revision.owner.as_deref(),
            Some("5ERcQ879d4HofcDgzTxaZXo98JNZvjKWQcisM8thyAkj8ztQ")
        );
        assert_eq!(revision.revision_id, Some(1));
        assert_eq!(
            revision.cid,
            "QmNLfbof5rLekrACjeuLk9JmGZD2HDBHCU4z16iYKmx5SE"
        );
    }

    #[test]
    fn extract_publish_revision_ignores_other_events() {
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

        assert_eq!(extract_publish_revision(&notification).unwrap(), None);
    }

    #[test]
    fn extract_publish_revision_errors_when_hash_missing() {
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
            extract_publish_revision(&notification),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn extract_image_cids_from_item_bytes_collects_primary_and_mipmaps() {
        let primary = digest_to_multihash_bytes(&[0x11; 32]);
        let mip_1 = digest_to_multihash_bytes(&[0x22; 32]);
        let mip_2 = digest_to_multihash_bytes(&[0x33; 32]);

        let bytes = ItemMessage {
            mixin_payload: vec![MixinPayloadMessage {
                mixin_id: IMAGE_MIXIN_ID,
                payload: ImageMixinMessage {
                    filename: "example.jpg".into(),
                    filesize: 123,
                    ipfs_hash: primary.clone(),
                    width: 640,
                    height: 480,
                    mipmap_level: vec![
                        MipmapLevelMessage {
                            filesize: 100,
                            ipfs_hash: mip_1.clone(),
                        },
                        MipmapLevelMessage {
                            filesize: 50,
                            ipfs_hash: mip_2.clone(),
                        },
                    ],
                }
                .encode_to_vec(),
            }],
        }
        .encode_to_vec();

        let cids = extract_image_cids_from_item_bytes(&bytes).unwrap();
        assert_eq!(
            cids,
            vec![
                multihash_bytes_to_cid(&primary),
                multihash_bytes_to_cid(&mip_1),
                multihash_bytes_to_cid(&mip_2),
            ]
        );
    }

    #[test]
    fn extract_image_cids_from_item_bytes_deduplicates_and_ignores_empty_hashes() {
        let shared = digest_to_multihash_bytes(&[0x44; 32]);

        let bytes = ItemMessage {
            mixin_payload: vec![
                MixinPayloadMessage {
                    mixin_id: 123,
                    payload: vec![1, 2, 3],
                },
                MixinPayloadMessage {
                    mixin_id: IMAGE_MIXIN_ID,
                    payload: ImageMixinMessage {
                        filename: String::new(),
                        filesize: 0,
                        ipfs_hash: Vec::new(),
                        width: 0,
                        height: 0,
                        mipmap_level: vec![
                            MipmapLevelMessage {
                                filesize: 1,
                                ipfs_hash: shared.clone(),
                            },
                            MipmapLevelMessage {
                                filesize: 2,
                                ipfs_hash: shared.clone(),
                            },
                            MipmapLevelMessage {
                                filesize: 3,
                                ipfs_hash: Vec::new(),
                            },
                        ],
                    }
                    .encode_to_vec(),
                },
            ],
        }
        .encode_to_vec();

        let cids = extract_image_cids_from_item_bytes(&bytes).unwrap();
        assert_eq!(cids, vec![multihash_bytes_to_cid(&shared)]);
    }
}
