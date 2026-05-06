use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::{
    cid::digest_hex_to_cid,
    error::Error,
    types::{
        DecodedChainEvent, IndexerMessage, JsonRpcPayload, JsonRpcResponse, MetadataResult,
        NotificationResult, PublishRevision, SubscriptionNotification,
    },
};

pub async fn close_indexer_connection<S>(
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

    match tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await {
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

pub async fn lookup_publish_revision_variant<S>(ws: &mut S) -> Result<(u8, u8), Error>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    const REQUEST_ID: u64 = 1;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": REQUEST_ID,
        "method": "acuity_getEventMetadata",
        "params": {}
    });
    ws.send(Message::Text(request.to_string())).await?;

    while let Some(message) = ws.next().await {
        let text = match message? {
            Message::Text(text) => text,
            _ => continue,
        };
        let Some(response) = parse_json_rpc_response_by_id::<MetadataResult>(&text, REQUEST_ID)?
        else {
            continue;
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
    const REQUEST_ID: u64 = 2;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": REQUEST_ID,
        "method": "acuity_subscribeEvents",
        "params": {
            "key": {
                "type": "Variant",
                "value": [pallet_index, event_index]
            }
        }
    });
    ws.send(Message::Text(request.to_string())).await?;

    while let Some(message) = ws.next().await {
        let text = match message? {
            Message::Text(text) => text,
            _ => continue,
        };
        let Some(response) = parse_json_rpc_response_by_id::<String>(&text, REQUEST_ID)? else {
            continue;
        };
        return Ok(response.result);
    }

    Err(Error::Protocol(
        "websocket closed before subscribe response arrived".into(),
    ))
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

    let DecodedChainEvent::ContentPublishRevision(fields) = &decoded_event.event else {
        return Ok(None);
    };

    info!(?fields, "decoded Content.PublishRevision fields");

    let ipfs_hash = fields.ipfs_hash.as_deref().ok_or_else(|| {
        Error::Protocol("Content.PublishRevision missing fields.ipfs_hash".into())
    })?;

    Ok(Some(PublishRevision {
        item_id: fields.item_id.clone(),
        owner: fields.owner.clone(),
        revision_id: fields.revision_id,
        cid: digest_hex_to_cid(ipfs_hash)?,
    }))
}

pub fn parse_indexer_message<T>(text: &str) -> Result<Option<IndexerMessage<T>>, Error>
where
    T: serde::de::DeserializeOwned,
{
    match serde_json::from_str(text) {
        Ok(message) => Ok(Some(message)),
        Err(_) => Ok(None),
    }
}

fn parse_json_rpc_response_by_id<T>(
    text: &str,
    expected_id: u64,
) -> Result<Option<JsonRpcResponse<T>>, Error>
where
    T: serde::de::DeserializeOwned,
{
    let Some(IndexerMessage::Response(response)) = parse_indexer_message::<T>(text)? else {
        return Ok(None);
    };

    if response.id != expected_id {
        return Ok(None);
    }

    match response.payload {
        JsonRpcPayload::Result { result } => Ok(Some(JsonRpcResponse {
            id: response.id,
            result,
        })),
        JsonRpcPayload::Error { error } => Err(Error::Protocol(format!(
            "json-rpc request {} failed (code {}): {}{}",
            expected_id,
            error.code,
            error.message,
            error
                .data
                .map(|data| format!("; data: {data}"))
                .unwrap_or_default()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_indexer_message, parse_json_rpc_response_by_id};
    use crate::{
        DecodedChainEvent, IndexerMessage, MetadataResult, PalletMeta, SubscriptionKey,
        SubscriptionNotification,
    };

    #[test]
    fn parse_json_rpc_response_by_id_ignores_other_request_ids() {
        let text = r#"{"jsonrpc":"2.0","id":99,"result":{"pallets":[]}}"#;

        let response = parse_json_rpc_response_by_id::<MetadataResult>(text, 1).unwrap();
        assert!(response.is_none());
    }

    #[test]
    fn parse_json_rpc_response_by_id_returns_matching_response() {
        let text = r#"{"jsonrpc":"2.0","id":1,"result":{"pallets":[{"index":4,"name":"Content","events":[]}]}}"#;

        let response = parse_json_rpc_response_by_id::<MetadataResult>(text, 1)
            .unwrap()
            .unwrap();
        assert_eq!(response.id, 1);
        assert_eq!(response.result.pallets.len(), 1);
        assert_eq!(
            response.result.pallets,
            vec![PalletMeta {
                index: 4,
                name: "Content".into(),
                events: vec![],
            }]
        );
    }

    #[test]
    fn parse_json_rpc_response_by_id_surfaces_matching_errors() {
        let text = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"boom"}}"#;

        let error = parse_json_rpc_response_by_id::<String>(text, 2).unwrap_err();
        assert!(matches!(error, crate::Error::Protocol(_)));
    }

    #[test]
    fn parse_indexer_message_parses_subscription_notifications() {
        let text = r#"{
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
                                "item_id": "0x01",
                                "owner": "5abc",
                                "revision_id": "7",
                                "ipfs_hash": "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
                            }
                        }
                    }
                }
            }
        }"#;

        let message = parse_indexer_message::<serde::de::IgnoredAny>(text)
            .unwrap()
            .unwrap();

        match message {
            IndexerMessage::Notification(notification) => match notification.params.result {
                crate::NotificationResult::Event { key, .. } => {
                    assert_eq!(key, SubscriptionKey::Variant { value: [4, 1] });
                }
                _ => panic!("expected event notification"),
            },
            _ => panic!("expected notification"),
        }
    }

    #[test]
    fn publish_revision_without_fields_deserializes_to_typed_empty_fields() {
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
                            "eventName": "PublishRevision"
                        }
                    }
                }
            }
        }))
        .unwrap();

        let crate::NotificationResult::Event {
            decoded_event: Some(decoded_event),
            ..
        } = notification.params.result
        else {
            panic!("expected event notification");
        };

        match decoded_event.event {
            DecodedChainEvent::ContentPublishRevision(fields) => {
                assert_eq!(fields.item_id, None);
                assert_eq!(fields.owner, None);
                assert_eq!(fields.revision_id, None);
                assert_eq!(fields.ipfs_hash, None);
            }
            _ => panic!("expected publish revision event"),
        }
    }
}
