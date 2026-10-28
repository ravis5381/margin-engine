use crate::DataError;
use margin_domain::Sha256Hex;
use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> Sha256Hex {
    let digest = Sha256::digest(bytes);
    let mut exact = [0_u8; 32];
    exact.copy_from_slice(&digest);
    Sha256Hex::from_digest(&exact)
}

pub fn enforce_size_limit(len: u64, max_bytes: u64) -> Result<(), DataError> {
    if len > max_bytes {
        return Err(DataError::ArchiveRejected {
            detail: format!("payload of {len} bytes exceeds the configured limit of {max_bytes}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{enforce_size_limit, sha256_hex};

    #[test]
    fn hash_is_stable_and_size_limit_fails_closed() {
        let digest = sha256_hex(b"synthetic");
        assert_eq!(digest.as_str().len(), 64);
        assert_eq!(sha256_hex(b"synthetic"), digest);
        assert!(enforce_size_limit(11, 10).is_err());
        assert!(enforce_size_limit(10, 10).is_ok());
    }
}
