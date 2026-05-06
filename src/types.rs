use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum IndexerMessage<T> {
    Response(JsonRpcMessage<T>),
    Notification(SubscriptionNotification),
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub id: u64,
    pub result: T,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcMessage<T> {
    pub id: u64,
    #[serde(flatten)]
    pub payload: JsonRpcPayload<T>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcPayload<T> {
    Result { result: T },
    Error { error: JsonRpcError },
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct MetadataResult {
    pub pallets: Vec<PalletMeta>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct PalletMeta {
    pub index: u8,
    pub name: String,
    pub events: Vec<EventMeta>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
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
        spans: Vec<serde::de::IgnoredAny>,
    },
    Event {
        key: SubscriptionKey,
        event: EventRef,
        #[serde(rename = "decodedEvent")]
        decoded_event: Option<DecodedEvent>,
    },
    Terminated {
        reason: String,
        message: String,
    },
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum SubscriptionKey {
    Variant {
        value: [u8; 2],
    },
    #[serde(other)]
    Unknown,
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
    pub event: DecodedChainEvent,
}

#[derive(Debug)]
pub enum DecodedChainEvent {
    ContentPublishRevision(PublishRevisionFields),
    Other {
        pallet_name: String,
        event_name: String,
    },
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PublishRevisionFields {
    pub item_id: Option<String>,
    pub owner: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u32")]
    pub revision_id: Option<u32>,
    pub ipfs_hash: Option<String>,
}

impl<'de> Deserialize<'de> for DecodedChainEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawDecodedChainEvent {
            #[serde(rename = "palletName")]
            pallet_name: String,
            #[serde(rename = "eventName")]
            event_name: String,
            #[serde(default)]
            fields: Option<PublishRevisionFields>,
        }

        let raw = RawDecodedChainEvent::deserialize(deserializer)?;

        if raw.pallet_name == "Content" && raw.event_name == "PublishRevision" {
            Ok(Self::ContentPublishRevision(raw.fields.unwrap_or_default()))
        } else {
            Ok(Self::Other {
                pallet_name: raw.pallet_name,
                event_name: raw.event_name,
            })
        }
    }
}

fn deserialize_optional_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("invalid u32 number")),
        Some(Value::String(string)) => string
            .parse::<u32>()
            .map(Some)
            .map_err(|error| serde::de::Error::custom(format!("invalid u32 string: {error}"))),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected u32 number or string, got {other}"
        ))),
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRevision {
    pub item_id: Option<String>,
    pub owner: Option<String>,
    pub revision_id: Option<u32>,
    pub cid: String,
}
