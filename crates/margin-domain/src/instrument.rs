use crate::error::DomainError;
use serde::{Deserialize, Serialize};

/// Underlying identifier carried on a contract. It is not a contract id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Underlying(String);

impl Underlying {
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() || value.len() > 32 {
            return Err(DomainError::InvalidContract {
                detail: "underlying must be 1 to 32 characters".to_owned(),
            });
        }
        if !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '&')
        }) {
            return Err(DomainError::InvalidContract {
                detail: format!("underlying `{value}` contains unsupported characters"),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Underlying {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Products whose margin treatment is in scope once their official methodology is loaded.
///
/// No additional product variant exists. Unrecognized source codes fail in [`InstrumentType::parse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum InstrumentType {
    #[serde(rename = "INDEX_FUTURE")]
    IndexFuture,
    #[serde(rename = "INDEX_OPTION")]
    IndexOption,
    #[serde(rename = "STOCK_FUTURE")]
    StockFuture,
    #[serde(rename = "STOCK_OPTION")]
    StockOption,
}

impl InstrumentType {
    pub const fn code(self) -> &'static str {
        match self {
            Self::IndexFuture => "INDEX_FUTURE",
            Self::IndexOption => "INDEX_OPTION",
            Self::StockFuture => "STOCK_FUTURE",
            Self::StockOption => "STOCK_OPTION",
        }
    }

    pub const fn is_option(self) -> bool {
        matches!(self, Self::IndexOption | Self::StockOption)
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "INDEX_FUTURE" => Ok(Self::IndexFuture),
            "INDEX_OPTION" => Ok(Self::IndexOption),
            "STOCK_FUTURE" => Ok(Self::StockFuture),
            "STOCK_OPTION" => Ok(Self::StockOption),
            other => Err(DomainError::UnsupportedProduct {
                detail: format!("instrument `{other}` is not supported"),
            }),
        }
    }
}

impl std::fmt::Display for InstrumentType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum OptionType {
    #[serde(rename = "CALL")]
    Call,
    #[serde(rename = "PUT")]
    Put,
}

impl OptionType {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Call => "CALL",
            Self::Put => "PUT",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "CALL" => Ok(Self::Call),
            "PUT" => Ok(Self::Put),
            other => Err(DomainError::InvalidContract {
                detail: format!("option type `{other}` is not CALL or PUT"),
            }),
        }
    }
}

impl std::fmt::Display for OptionType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}
