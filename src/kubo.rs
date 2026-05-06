use std::{collections::BTreeSet, net::IpAddr, path::PathBuf, process::Stdio};

use anyhow::{Result, anyhow};
use tokio::{
    process::{Child, Command},
    time::{Duration, Instant},
};
use tracing::{info, warn};

use crate::types::KuboIdResponse;

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

    pub async fn id(&self) -> Result<KuboIdResponse> {
        let url = format!("{}/api/v0/id", self.base_url);
        let timeout = std::time::Duration::from_secs(10);

        let response = self.http.post(&url).timeout(timeout).send().await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            warn!(%url, %status, body = %body, "kubo id request failed");
            return Err(anyhow!("kubo id failed with status {status}: {body}"));
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

    pub async fn pin(&self, cid: &str) -> Result<()> {
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
            return Err(anyhow!("kubo pin/add failed with status {status}: {body}"));
        }

        info!(cid = %cid, %url, %status, body = %body, "kubo pin/add completed");
        Ok(())
    }

    pub async fn cat(&self, cid: &str) -> Result<Vec<u8>> {
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
            return Err(anyhow!("kubo cat failed with status {status}: {body}"));
        }

        let bytes = response.bytes().await?.to_vec();
        info!(cid = %cid, %url, %status, byte_len = bytes.len(), "kubo cat completed");
        Ok(bytes)
    }
}

fn resolve_kubo_repo_dir() -> Result<PathBuf> {
    match home::home_dir() {
        Some(mut path) => {
            path.push(".local/share/acuity-ipfs-pinner");
            path.push("ipfs-repo");
            Ok(path)
        }
        None => Err(anyhow!("no home directory")),
    }
}

async fn ensure_kubo_repo_initialized(repo_dir: &std::path::Path) -> Result<()> {
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
        _ => Err(anyhow!(
            "ipfs init failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )),
    }
}

async fn configure_kubo_swarm_addresses(repo_dir: &std::path::Path) -> Result<()> {
    const SWARM_ADDRESSES: &str = r#"[
  \"/ip4/0.0.0.0/tcp/4001\",
  \"/ip4/0.0.0.0/tcp/4002/ws\",
  \"/ip6/::/tcp/4001\",
  \"/ip6/::/tcp/4002/ws\",
  \"/ip4/0.0.0.0/udp/4001/webrtc-direct\",
  \"/ip4/0.0.0.0/udp/4001/quic-v1\",
  \"/ip4/0.0.0.0/udp/4001/quic-v1/webtransport\",
  \"/ip6/::/udp/4001/webrtc-direct\",
  \"/ip6/::/udp/4001/quic-v1\",
  \"/ip6/::/udp/4001/quic-v1/webtransport\"
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
        Err(anyhow!(
            "ipfs config Addresses.Swarm failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
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

    let ws_index = address.find("/ws")?;

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

async fn wait_for_kubo_api(kubo: &KuboClient, daemon: &mut Child) -> Result<KuboIdResponse> {
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        if let Some(status) = daemon.try_wait()? {
            return Err(anyhow!(
                "ipfs daemon exited before becoming available: {status}"
            ));
        }

        match kubo.id().await {
            Ok(id) => return Ok(id),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(anyhow!("timed out waiting for kubo api: {error}"));
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

pub async fn start_kubo_daemon(kubo: &KuboClient) -> Result<Option<Child>> {
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

pub async fn stop_kubo_daemon(daemon: &mut Child) -> Result<()> {
    if let Some(status) = daemon.try_wait()? {
        info!(status = %status, "ipfs daemon already exited");
        return Ok(());
    }

    let Some(pid) = daemon.id() else {
        return Err(anyhow!("ipfs daemon pid unavailable"));
    };

    info!(pid, "sending SIGINT to ipfs daemon");
    let status = Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("failed to send SIGINT to ipfs daemon: {status}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_kubo_repo_dir_uses_expected_suffix() {
        let path = resolve_kubo_repo_dir().unwrap();
        assert!(path.ends_with(".local/share/acuity-ipfs-pinner/ipfs-repo"));
    }

    #[test]
    fn is_local_ip_classifies_local_and_external_addresses() {
        assert!(is_local_ip("127.0.0.1".parse().unwrap()));
        assert!(is_local_ip("10.0.0.1".parse().unwrap()));
        assert!(is_local_ip("169.254.1.1".parse().unwrap()));
        assert!(is_local_ip("0.0.0.0".parse().unwrap()));
        assert!(is_local_ip("::1".parse().unwrap()));
        assert!(is_local_ip("fc00::1".parse().unwrap()));
        assert!(is_local_ip("fe80::1".parse().unwrap()));
        assert!(!is_local_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_local_ip("2001:4860:4860::8888".parse().unwrap()));
    }

    #[test]
    fn normalize_ws_multiaddr_accepts_supported_addresses() {
        assert_eq!(
            normalize_ws_multiaddr("/ip4/127.0.0.1/tcp/4002/ws", "peer"),
            Some("/ip4/127.0.0.1/tcp/4002/ws/p2p/peer".into())
        );
        assert_eq!(
            normalize_ws_multiaddr("/ip6/::1/tcp/4002/ws/p2p/old", "peer"),
            Some("/ip6/::1/tcp/4002/ws/p2p/peer".into())
        );
        assert_eq!(
            normalize_ws_multiaddr("/ip4/127.0.0.1/tcp/4002/ws/extra", "peer"),
            Some("/ip4/127.0.0.1/tcp/4002/ws/extra/p2p/peer".into())
        );
    }

    #[test]
    fn normalize_ws_multiaddr_rejects_invalid_addresses() {
        assert_eq!(normalize_ws_multiaddr("/dns4/example.com/tcp/4002/ws", "peer"), None);
        assert_eq!(normalize_ws_multiaddr("/ip4/not-an-ip/tcp/4002/ws", "peer"), None);
        assert_eq!(normalize_ws_multiaddr("/ip4/127.0.0.1/tcp/4002/wss", "peer"), None);
        assert_eq!(normalize_ws_multiaddr("/ip4/127.0.0.1/tcp/4002", "peer"), None);
    }
}
