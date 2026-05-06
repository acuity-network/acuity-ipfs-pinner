mod cid;
mod cli;
mod config;
mod error;
mod indexer;
mod kubo;
mod protobuf;
mod service;
mod types;

pub use cid::{digest_hex_to_cid, hex_to_bytes32};
pub use cli::Cli;
pub use config::{Config, DEFAULT_INDEXER_URL, DEFAULT_KUBO_API_URL};
pub use error::Error;
pub use indexer::{
    close_indexer_connection, extract_publish_revision, lookup_publish_revision_variant,
    subscribe_to_variant,
};
pub use kubo::{start_kubo_daemon, stop_kubo_daemon, KuboClient};
pub use protobuf::{ImageMixinMessage, ItemMessage, MixinPayloadMessage, MipmapLevelMessage, IMAGE_MIXIN_ID};
pub use service::run;
pub use types::{
    DecodedEvent, EventMeta, EventRef, JsonRpcResponse, KuboIdResponse, MetadataResult,
    NotificationParams, NotificationResult, PalletMeta, PublishRevision,
    SubscriptionNotification,
};

#[cfg(test)]
mod tests {
    use prost::Message as ProstMessage;

    use super::{
        cid::{digest_to_multihash_bytes, multihash_bytes_to_cid},
        protobuf::extract_image_cids_from_item_bytes,
        *,
    };

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
