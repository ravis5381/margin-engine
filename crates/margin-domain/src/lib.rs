//! Domain model for NSE and BSE equity-derivatives margin calculations.
//!
//! Monetary amounts use [`rust_decimal::Decimal`]. This crate has no official
//! SPAN implementation and does not claim equivalence with NSE Clearing or ICCL.

mod clearing;
mod contract;
mod dataset;
mod error;
mod ids;
mod instrument;
mod money;
mod position;
mod quantity;
mod risk_array;

pub use clearing::{
    ClearingArrangement, ClearingConfiguration, ClearingCorporation, Exchange, OffsetEligibility,
    assess_cross_exchange_offset,
};
pub use contract::{Contract, ContractDraft, LotSize, Multiplier};
pub use dataset::{
    DatasetOrigin, DatasetQuery, DatasetSelector, IngestDisposition, PublicationStatus,
    RiskDataset, RiskDatasetDraft, SnapshotClass, SnapshotLocator, ValidationStatus,
    classify_ingest, select_dataset,
};
pub use error::DomainError;
pub use ids::{
    AggregationScopeId, CalculationId, ContractId, ContractMasterVersionId, DatasetId, PortfolioId,
    RuleVersionId, ScenarioId, Sha256Hex, SourceContractId,
};
pub use instrument::{InstrumentType, OptionType, Underlying};
pub use money::{Currency, Money, RoundingMode, RoundingPolicy};
pub use position::{
    AggregationScope, AggregationScopeKind, CalculationScope, NormalizedBook, Position,
    PositionDraft, ValuationPrice,
};
pub use quantity::{QuantityConversion, QuantityUnit, SignedQuantity};
pub use risk_array::{
    NormalizedRiskArray, NormalizedScenario, RiskArrayDraft, ScenarioLayoutRegistry,
    ScenarioLayoutSpec, ScenarioValueDraft, ValueConvention,
};

/// Reported on API responses. This build has not been reconciled to a clearing corporation.
pub const EQUIVALENCE_CLAIM: &str = "none";

/// Full statement used by logs, documentation, and data-status responses.
pub const EQUIVALENCE_STATEMENT: &str = "This build does not replicate NSE Clearing or ICCL margin calculations. Official methodologies are not implemented and have not been reconciled.";

/// Official production margin calculations implemented in this build.
///
/// Diagnostic scenario arithmetic is intentionally excluded.
pub const OFFICIAL_CALCULATION_IMPLEMENTED: bool = false;
