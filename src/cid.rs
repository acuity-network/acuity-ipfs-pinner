use anyhow::{Result, anyhow};

pub fn hex_to_bytes32(hex_value: &str) -> Result<[u8; 32]> {
    let raw = hex::decode(hex_value.trim_start_matches("0x"))
        .map_err(|error| anyhow!("invalid hex value {hex_value}: {error}"))?;
    raw.try_into()
        .map_err(|_| anyhow!("expected 32 bytes for {hex_value}"))
}

pub fn digest_hex_to_cid(hex_value: &str) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_to_bytes32_accepts_prefixed_and_unprefixed_hex() {
        let prefixed = hex_to_bytes32(
            "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .unwrap();
        let unprefixed =
            hex_to_bytes32("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
                .unwrap();
        assert_eq!(prefixed, unprefixed);
        assert_eq!(prefixed[0], 0);
        assert_eq!(prefixed[31], 31);
    }

    #[test]
    fn hex_to_bytes32_rejects_invalid_hex() {
        let error = hex_to_bytes32("0xnot-hex").unwrap_err();
        assert!(error.to_string().contains("invalid hex value"));
    }

    #[test]
    fn hex_to_bytes32_rejects_wrong_length() {
        let error = hex_to_bytes32("0x01").unwrap_err();
        assert!(error.to_string().contains("expected 32 bytes"));
    }

    #[test]
    fn digest_to_multihash_prepends_sha256_prefix() {
        let digest = [0xaa; 32];
        let multihash = digest_to_multihash_bytes(&digest);
        assert_eq!(multihash.len(), 34);
        assert_eq!(&multihash[..2], &[0x12, 0x20]);
        assert_eq!(&multihash[2..], &digest);
    }
}
