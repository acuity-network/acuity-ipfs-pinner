use std::collections::BTreeSet;

use anyhow::Result;
use prost::Message as ProstMessage;

use crate::cid::multihash_bytes_to_cid;

pub const IMAGE_MIXIN_ID: u32 = 0x045e_ee8c;

#[derive(Clone, PartialEq, prost::Message)]
pub struct ItemMessage {
    #[prost(uint32, tag = "1")]
    pub content_type_id: u32,
    #[prost(message, repeated, tag = "2")]
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

pub(crate) fn extract_image_cids_from_item_bytes(bytes: &[u8]) -> Result<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use prost::Message as ProstMessage;

    use super::*;

    #[test]
    fn extract_image_cids_from_item_bytes_rejects_invalid_item_bytes() {
        let error = extract_image_cids_from_item_bytes(&[0xff]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to decode Protobuf message")
        );
    }

    #[test]
    fn extract_image_cids_from_item_bytes_rejects_invalid_image_payload() {
        let bytes = ItemMessage {
            content_type_id: 0,
            mixin_payload: vec![MixinPayloadMessage {
                mixin_id: IMAGE_MIXIN_ID,
                payload: vec![0xff],
            }],
        }
        .encode_to_vec();

        let error = extract_image_cids_from_item_bytes(&bytes).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to decode Protobuf message")
        );
    }
}
