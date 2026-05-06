use crate::error::Error;

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

pub(crate) fn digest_to_multihash_bytes(digest: &[u8; 32]) -> Vec<u8> {
    let mut multihash = Vec::with_capacity(34);
    multihash.push(0x12);
    multihash.push(0x20);
    multihash.extend_from_slice(digest);
    multihash
}

pub(crate) fn multihash_bytes_to_cid(multihash: &[u8]) -> String {
    bs58::encode(multihash).into_string()
}
