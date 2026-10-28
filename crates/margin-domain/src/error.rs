use crate::clearing::{ClearingCorporation, Exchange};
use crate::money::Currency;
use crate::quantity::QuantityUnit;
use chrono::NaiveDate;

/// Recoverable domain failure. Library code returns these instead of substituting data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("currency mismatch: {left} and {right}")]
    CurrencyMismatch { left: Currency, right: Currency },

    #[error("quantity unit mismatch: {left} and {right}")]
    QuantityUnitMismatch {
        left: QuantityUnit,
        right: QuantityUnit,
    },

    #[error("monetary arithmetic overflowed")]
    MonetaryOverflow,

    #[error("invalid monetary amount: {detail}")]
    InvalidAmount { detail: String },

    #[error("invalid identifier: {detail}")]
    InvalidIdentifier { detail: String },

    #[error("invalid contract: {detail}")]
    InvalidContract { detail: String },

    #[error("quantity conversion rejected: {detail}")]
    QuantityConversionRejected { detail: String },

    #[error("conflicting position attributes for contract {contract_id}: {detail}")]
    ConflictingPositionAttributes { contract_id: String, detail: String },

    #[error("missing required data: {detail}")]
    MissingRequiredData { detail: String },

    #[error("unsupported product: {detail}")]
    UnsupportedProduct { detail: String },

    #[error(
        "clearing arrangement is not configured for exchange {exchange}, clearing corporation {clearing_corporation}, trade date {trade_date}"
    )]
    ClearingArrangementRejected {
        exchange: Exchange,
        clearing_corporation: ClearingCorporation,
        trade_date: NaiveDate,
    },

    #[error("risk dataset is not usable: {detail}")]
    DatasetNotUsable { detail: String },

    #[error(
        "a newer risk dataset for the same date and snapshot is not usable, so no older dataset was selected: {detail}"
    )]
    NewerDatasetNotUsable { detail: String },

    #[error("risk dataset selection is ambiguous: {detail}")]
    AmbiguousDataset { detail: String },

    #[error("cross-exchange or cross-clearing offset rejected: {detail}")]
    OffsetRejected { detail: String },

    #[error("risk array rejected: {detail}")]
    RiskArrayRejected { detail: String },

    #[error("stale or mismatched risk data: {detail}")]
    StaleRiskData { detail: String },
}
