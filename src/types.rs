use serde::Deserialize;
use serde_json::Value;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRevision {
    pub item_id: Option<String>,
    pub owner: Option<String>,
    pub revision_id: Option<u32>,
    pub cid: String,
}
