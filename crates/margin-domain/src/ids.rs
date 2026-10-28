use crate::error::DomainError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_TOKEN_LEN: usize = 64;

fn parse_token(value: &str, what: &str) -> Result<String, DomainError> {
    if value.is_empty() || value.len() > MAX_TOKEN_LEN {
        return Err(DomainError::InvalidIdentifier {
            detail: format!("{what} must be 1 to {MAX_TOKEN_LEN} characters"),
        });
    }
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
    }) {
        return Err(DomainError::InvalidIdentifier {
            detail: format!(
                "{what} contains characters outside ASCII letters, digits, '-', '_', '.', and ':'"
            ),
        });
    }
    Ok(value.to_owned())
}

macro_rules! token_id {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: &str) -> Result<Self, DomainError> {
                Ok(Self(parse_token(value, $label)?))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

token_id!(ContractId, "contract id");
token_id!(SourceContractId, "source contract id");
token_id!(PortfolioId, "portfolio id");
token_id!(AggregationScopeId, "aggregation scope id");
token_id!(ScenarioId, "scenario id");
token_id!(RuleVersionId, "rule version id");

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }
    };
}

uuid_id!(DatasetId);
uuid_id!(ContractMasterVersionId);
uuid_id!(CalculationId);

/// Lower-case SHA-256 hex digest. The value is the identity of archived bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256Hex(String);

impl Sha256Hex {
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.len() != 64
            || !normalized
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(DomainError::InvalidIdentifier {
                detail: "sha256 must be 64 hexadecimal characters".to_owned(),
            });
        }
        Ok(Self(normalized))
    }

    pub fn from_digest(bytes: &[u8; 32]) -> Self {
        Self(encode_hex(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Sha256Hex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Sha256Hex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::Sha256Hex;

    #[test]
    fn sha256_rejects_short_and_upper_case_is_normalized() {
        assert!(Sha256Hex::parse("abcd").is_err());
        let parsed = Sha256Hex::parse(&"AB".repeat(32)).expect("hex");
        assert_eq!(parsed.as_str(), "ab".repeat(32));
    }
}
