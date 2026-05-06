use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::{
    cid::digest_hex_to_cid,
    error::Error,
    types::{
        JsonRpcResponse, MetadataResult, NotificationResult, PublishRevision,
        SubscriptionNotification,
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
        .ok_or_else(|| Error::Protocol("Content.PublishRevision missing fields.ipfs_hash".into()))?;

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
