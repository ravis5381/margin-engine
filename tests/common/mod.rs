#![allow(dead_code)]

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use margin_domain::{
    AggregationScope, AggregationScopeId, AggregationScopeKind, ClearingCorporation, Contract,
    ContractDraft, ContractId, ContractMasterVersionId, Currency, DatasetId, DatasetOrigin,
    Exchange, InstrumentType, LotSize, Money, Multiplier, NormalizedBook, NormalizedRiskArray,
    OptionType, PortfolioId, Position, PositionDraft, PublicationStatus, QuantityUnit,
    RiskArrayDraft, RiskDataset, RiskDatasetDraft, ScenarioId, ScenarioValueDraft, Sha256Hex,
    SignedQuantity, SnapshotLocator, SourceContractId, Underlying, ValidationStatus,
    ValueConvention,
};
use rust_decimal::Decimal;

pub fn trade_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 28).expect("date")
}

pub fn at_hour(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 28, hour, 0, 0)
        .single()
        .expect("timestamp")
}

pub fn sha(byte: u8) -> Sha256Hex {
    Sha256Hex::parse(&format!("{byte:02x}{}", "cd".repeat(31))).expect("sha")
}

pub struct DatasetParts {
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub trading_date: NaiveDate,
    pub snapshot: SnapshotLocator,
    pub publication_timestamp: Option<DateTime<Utc>>,
    pub download_timestamp: DateTime<Utc>,
    pub sha_byte: u8,
    pub origin: DatasetOrigin,
    pub source_host: Option<String>,
    pub validation_status: ValidationStatus,
    pub validation_error_summary: Option<String>,
    pub publication_status: PublicationStatus,
    pub contract_master_version_id: Option<ContractMasterVersionId>,
}

impl DatasetParts {
    pub fn published(
        exchange: Exchange,
        clearing_corporation: ClearingCorporation,
        hour: u32,
        sha_byte: u8,
    ) -> Self {
        Self {
            exchange,
            clearing_corporation,
            trading_date: trade_date(),
            snapshot: SnapshotLocator::end_of_day(),
            publication_timestamp: Some(at_hour(hour)),
            download_timestamp: at_hour(hour),
            sha_byte,
            origin: DatasetOrigin::Synthetic,
            source_host: None,
            validation_status: ValidationStatus::Passed,
            validation_error_summary: None,
            publication_status: PublicationStatus::Published,
            contract_master_version_id: Some(ContractMasterVersionId::from_uuid(
                uuid::Uuid::from_u128(1),
            )),
        }
    }
}

pub fn dataset(parts: DatasetParts) -> RiskDataset {
    RiskDataset::try_new(RiskDatasetDraft {
        id: DatasetId::new(),
        exchange: parts.exchange,
        clearing_corporation: parts.clearing_corporation,
        trading_date: parts.trading_date,
        snapshot: parts.snapshot,
        publication_timestamp: parts.publication_timestamp,
        download_timestamp: parts.download_timestamp,
        original_filename: format!("synthetic-{}.dat", parts.sha_byte),
        source_host: parts.source_host,
        sha256: sha(parts.sha_byte),
        file_format: "synthetic".to_owned(),
        parser_version: "none".to_owned(),
        contract_master_version_id: parts.contract_master_version_id,
        validation_status: parts.validation_status,
        validation_error_summary: parts.validation_error_summary,
        publication_status: parts.publication_status,
        origin: parts.origin,
    })
    .expect("dataset")
}

pub fn scope() -> AggregationScope {
    AggregationScope::new(
        AggregationScopeId::parse("ACC-1").expect("scope"),
        AggregationScopeKind::ClientAccount,
    )
    .expect("scope")
}

pub fn position(
    contract_id: &str,
    exchange: Exchange,
    clearing_corporation: ClearingCorporation,
    quantity: i64,
    unit: QuantityUnit,
) -> Position {
    Position::try_new(PositionDraft {
        portfolio_id: PortfolioId::parse("PORTFOLIO-001").expect("portfolio"),
        aggregation_scope: scope(),
        contract_id: ContractId::parse(contract_id).expect("contract"),
        exchange,
        quantity: SignedQuantity::new(Decimal::from(quantity), unit),
        average_trade_price: None,
        valuation: None,
        origin: "test-book".to_owned(),
        timestamp: at_hour(9),
        clearing_corporation,
        trading_date: trade_date(),
    })
    .expect("position")
}

pub fn pnl_array(
    contract_id: &str,
    dataset_id: DatasetId,
    values: &[(&str, &str)],
    unit: QuantityUnit,
) -> NormalizedRiskArray {
    let scenarios = values
        .iter()
        .enumerate()
        .map(|(index, (scenario_id, amount))| ScenarioValueDraft {
            scenario_id: ScenarioId::parse(scenario_id).expect("scenario"),
            official_index: u16::try_from(index).expect("index"),
            amount: Money::parse(amount, Currency::Inr).expect("amount"),
            convention: ValueConvention::ProfitAndLoss,
            quantity_basis: unit,
            scale: Decimal::ONE,
            source_raw: (*amount).to_owned(),
            source_locator: Some(format!("synthetic-row-{index}")),
        })
        .collect();
    NormalizedRiskArray::try_new(RiskArrayDraft {
        contract_id: ContractId::parse(contract_id).expect("contract"),
        dataset_id,
        quantity_basis: unit,
        scenarios,
    })
    .expect("array")
}

pub fn book(positions: &[Position]) -> NormalizedBook {
    NormalizedBook::from_positions(positions).expect("book")
}

pub fn index_future() -> Contract {
    Contract::try_new(ContractDraft {
        id: ContractId::parse("SYN-NIFTY-FUT").expect("id"),
        exchange: Exchange::Nse,
        clearing_corporation: ClearingCorporation::Nscc,
        underlying: Underlying::parse("NIFTY").expect("underlying"),
        instrument_type: InstrumentType::IndexFuture,
        expiry: NaiveDate::from_ymd_opt(2026, 11, 24).expect("expiry"),
        strike: None,
        option_type: None,
        lot_size: LotSize::new(65).expect("lot"),
        multiplier: Multiplier::new(Decimal::ONE).expect("multiplier"),
        currency: Currency::Inr,
        source_contract_id: SourceContractId::parse("SRC-NIFTY-FUT").expect("source"),
        effective_trading_date: trade_date(),
        contract_master_version: ContractMasterVersionId::from_uuid(uuid::Uuid::from_u128(1)),
    })
    .expect("contract")
}

pub fn index_option() -> Contract {
    Contract::try_new(ContractDraft {
        id: ContractId::parse("SYN-SENSEX-OPT").expect("id"),
        exchange: Exchange::Bse,
        clearing_corporation: ClearingCorporation::Iccl,
        underlying: Underlying::parse("SENSEX").expect("underlying"),
        instrument_type: InstrumentType::IndexOption,
        expiry: NaiveDate::from_ymd_opt(2026, 11, 26).expect("expiry"),
        strike: Some(Money::parse("80000", Currency::Inr).expect("strike")),
        option_type: Some(OptionType::Call),
        lot_size: LotSize::new(20).expect("lot"),
        multiplier: Multiplier::new(Decimal::ONE).expect("multiplier"),
        currency: Currency::Inr,
        source_contract_id: SourceContractId::parse("SRC-SENSEX-CALL").expect("source"),
        effective_trading_date: trade_date(),
        contract_master_version: ContractMasterVersionId::from_uuid(uuid::Uuid::from_u128(2)),
    })
    .expect("contract")
}
