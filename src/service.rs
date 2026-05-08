use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use tokio::{signal, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::{
    ack::start_ack_listener,
    config::Config,
    indexer::{
        close_indexer_connection, extract_publish_revision, lookup_publish_revision_variant,
        parse_indexer_message, subscribe_to_variant,
    },
    kubo::{KuboClient, start_kubo_daemon, stop_kubo_daemon},
    protobuf::extract_image_cids_from_item_bytes,
    types::{IndexerMessage, PublishRevision},
};

pub async fn run(config: Config) -> Result<()> {
    let pinner = KuboClient::new(config.kubo_api_url.clone());
    let mut kubo_daemon = start_kubo_daemon(&pinner).await?;
    let ack_listener = start_ack_listener(pinner.clone(), config.ack_protocol.clone()).await?;

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

    ack_listener.stop(&pinner).await?;

    if let Some(mut daemon) = kubo_daemon.take() {
        stop_kubo_daemon(&mut daemon).await?;
    }

    Ok(())
}

async fn run_once(config: &Config, pinner: KuboClient) -> Result<()> {
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
                    match parse_indexer_message::<serde::de::IgnoredAny>(&text) {
                        Ok(Some(IndexerMessage::Notification(notification))) => {
                            info!(
                                subscription = %notification.params.subscription,
                                method = %notification.method,
                                payload = %text,
                                "received subscription notification"
                            );

                            match extract_publish_revision(&notification) {
                                Ok(Some(revision)) => {
                                    log_queued_revision(&revision);

                                    let pin_client = pinner.clone();
                                    tokio::spawn(async move {
                                        pin_revision_and_images(pin_client, revision).await;
                                    });
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    warn!(error = %error, payload = %text, "ignoring malformed publish revision event");
                                }
                            }
                        }
                        Ok(Some(IndexerMessage::Response(_))) | Ok(None) => {
                            // Ignore JSON-RPC responses and unrelated messages.
                        }
                        Err(error) => {
                            warn!(error = %error, payload = %text, "ignoring malformed indexer message");
                        }
                    }
                }
            }
        }
    }

    Err(anyhow!("indexer websocket closed"))
}

fn log_queued_revision(revision: &PublishRevision) {
    info!(
        item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
        owner = revision.owner.as_deref().unwrap_or("<missing>"),
        revision_id = revision.revision_id.unwrap_or_default(),
        cid = %revision.cid,
        "queueing background pin"
    );
}

async fn pin_revision_and_images(pin_client: KuboClient, revision: PublishRevision) {
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
                        log_and_pin_image_cids(&pin_client, &revision, image_cids).await
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
}

async fn log_and_pin_image_cids(
    pin_client: &KuboClient,
    revision: &PublishRevision,
    image_cids: Vec<String>,
) {
    if image_cids.is_empty() {
        info!(
            item_id = revision.item_id.as_deref().unwrap_or("<missing>"),
            owner = revision.owner.as_deref().unwrap_or("<missing>"),
            revision_id = revision.revision_id.unwrap_or_default(),
            cid = %revision.cid,
            "no image mixin CIDs found in pinned content"
        );
        return;
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_queued_revision_handles_missing_fields() {
        log_queued_revision(&PublishRevision {
            item_id: None,
            owner: None,
            revision_id: None,
            cid: "QmParent".into(),
        });
    }

    #[test]
    fn log_queued_revision_handles_present_fields() {
        log_queued_revision(&PublishRevision {
            item_id: Some("item".into()),
            owner: Some("owner".into()),
            revision_id: Some(7),
            cid: "QmParent".into(),
        });
    }
}
