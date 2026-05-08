use std::net::SocketAddr;

use anyhow::{Result, anyhow};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::{Duration, sleep, timeout},
};
use tracing::{info, warn};

use crate::kubo::KuboClient;

const ACK_RESPONSE_PREFIX: &str = "ACK: received ";

pub struct AckListenerHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
    protocol: String,
}

impl AckListenerHandle {
    pub async fn stop(mut self, kubo: &KuboClient) -> Result<()> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        match self.join_handle.await {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(anyhow!("ack listener task failed: {error}")),
        }

        kubo.p2p_close(&self.protocol).await?;
        Ok(())
    }
}

pub async fn start_ack_listener(kubo: KuboClient, protocol: String) -> Result<AckListenerHandle> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let listen_addr = listener.local_addr()?;
    let target = format!("/ip4/{}/tcp/{}", listen_addr.ip(), listen_addr.port());

    kubo.p2p_close(&protocol).await.ok();
    kubo.p2p_listen(&protocol, &target).await?;

    info!(
        ack_protocol = %protocol,
        forward_target = %target,
        "registered kubo p2p ack listener"
    );

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let listener_protocol = protocol.clone();
    let join_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, remote_addr)) => {
                            let kubo = kubo.clone();
                            let protocol = listener_protocol.clone();
                            tokio::spawn(async move {
                                if let Err(error) = handle_ack_stream(kubo, &protocol, stream, remote_addr).await {
                                    warn!(ack_protocol = %protocol, remote_addr = %remote_addr, error = %error, "ack stream failed");
                                }
                            });
                        }
                        Err(error) => {
                            warn!(ack_protocol = %listener_protocol, error = %error, "ack listener accept failed");
                        }
                    }
                }
            }
        }
    });

    Ok(AckListenerHandle {
        shutdown_tx: Some(shutdown_tx),
        join_handle,
        protocol,
    })
}

async fn handle_ack_stream(
    kubo: KuboClient,
    protocol: &str,
    stream: TcpStream,
    remote_addr: SocketAddr,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut cid_bytes = Vec::new();
    timeout(Duration::from_secs(30), reader.read_until(b'\n', &mut cid_bytes)).await??;

    let cid = parse_cid_request(&cid_bytes)?;
    info!(ack_protocol = %protocol, remote_addr = %remote_addr, cid = %cid, "received CID push notification");

    let bytes = kubo.cat(&cid).await?;
    info!(ack_protocol = %protocol, remote_addr = %remote_addr, cid = %cid, byte_len = bytes.len(), "received content for CID; sending ack");

    let mut stream = reader.into_inner();
    let ack = format!("{ACK_RESPONSE_PREFIX}{cid}\n");
    timeout(Duration::from_secs(10), stream.write_all(ack.as_bytes())).await??;
    timeout(Duration::from_secs(10), stream.flush()).await??;
    sleep(Duration::from_millis(250)).await;
    timeout(Duration::from_secs(10), stream.shutdown()).await??;
    Ok(())
}

fn parse_cid_request(bytes: &[u8]) -> Result<String> {
    let cid = std::str::from_utf8(bytes)?.trim();
    if cid.is_empty() {
        return Err(anyhow!("ack stream payload was empty"));
    }

    Ok(cid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cid_request_trims_whitespace() {
        assert_eq!(parse_cid_request(b"  QmExample\n").unwrap(), "QmExample");
    }

    #[test]
    fn parse_cid_request_accepts_newline_delimited_payload() {
        assert_eq!(parse_cid_request(b"QmExample\nrest ignored by reader").unwrap(), "QmExample\nrest ignored by reader".trim());
    }

    #[test]
    fn parse_cid_request_rejects_empty_payload() {
        let error = parse_cid_request(b" \n\t ").unwrap_err();
        assert!(error.to_string().contains("empty"));
    }

    #[test]
    fn parse_cid_request_rejects_invalid_utf8() {
        assert!(parse_cid_request(&[0xff]).is_err());
    }
}
