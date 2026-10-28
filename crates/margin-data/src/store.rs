use crate::DataError;
use crate::span_quote::MarginPosition;
use margin_domain::{
    ClearingCorporation, DatasetId, DatasetOrigin, Exchange, RiskDataset, SnapshotClass,
    ValidationStatus,
};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetSummary {
    pub id: DatasetId,
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub trading_date: chrono::NaiveDate,
    pub snapshot_class: SnapshotClass,
    pub intraday_sequence: Option<u32>,
    pub validation_status: ValidationStatus,
    pub publication_status: String,
    pub origin: DatasetOrigin,
    pub sha256: String,
    pub file_format: String,
    pub parser_version: String,
    pub original_filename: String,
    pub validation_error_summary: Option<String>,
    pub contract_master_version_id: Option<margin_domain::ContractMasterVersionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetPage {
    pub datasets: Vec<DatasetSummary>,
    pub truncated: bool,
}

const PAGE_LIMIT: usize = 500;

pub fn summarize(dataset: &RiskDataset) -> DatasetSummary {
    DatasetSummary {
        id: dataset.id(),
        exchange: dataset.exchange(),
        clearing_corporation: dataset.clearing_corporation(),
        trading_date: dataset.trading_date(),
        snapshot_class: dataset.snapshot().class(),
        intraday_sequence: dataset.snapshot().intraday_sequence(),
        validation_status: dataset.validation_status(),
        publication_status: dataset.publication_status().code().to_owned(),
        origin: dataset.origin(),
        sha256: dataset.sha256().as_str().to_owned(),
        file_format: dataset.file_format().to_owned(),
        parser_version: dataset.parser_version().to_owned(),
        original_filename: dataset.original_filename().to_owned(),
        validation_error_summary: dataset.validation_error_summary().map(str::to_owned),
        contract_master_version_id: dataset.contract_master_version_id(),
    }
}

fn page_from(datasets: &[RiskDataset]) -> DatasetPage {
    let truncated = datasets.len() > PAGE_LIMIT;
    DatasetPage {
        datasets: datasets.iter().take(PAGE_LIMIT).map(summarize).collect(),
        truncated,
    }
}

pub trait DatasetReadModel: Send + Sync {
    fn ping(&self) -> impl std::future::Future<Output = Result<(), DataError>> + Send;
    fn validated_dataset_count(
        &self,
    ) -> impl std::future::Future<Output = Result<u64, DataError>> + Send;
    fn list_datasets(
        &self,
    ) -> impl std::future::Future<Output = Result<DatasetPage, DataError>> + Send;
    fn get_dataset(
        &self,
        id: DatasetId,
    ) -> impl std::future::Future<Output = Result<Option<RiskDataset>, DataError>> + Send;
}

#[derive(Clone)]
pub struct MemoryStore {
    datasets: Arc<Vec<RiskDataset>>,
}

impl MemoryStore {
    pub fn new(datasets: Vec<RiskDataset>) -> Self {
        Self {
            datasets: Arc::new(datasets),
        }
    }
}

impl DatasetReadModel for MemoryStore {
    async fn ping(&self) -> Result<(), DataError> {
        Ok(())
    }

    async fn validated_dataset_count(&self) -> Result<u64, DataError> {
        Ok(official_validated(&self.datasets))
    }

    async fn list_datasets(&self) -> Result<DatasetPage, DataError> {
        Ok(page_from(&self.datasets))
    }

    async fn get_dataset(&self, id: DatasetId) -> Result<Option<RiskDataset>, DataError> {
        Ok(self
            .datasets
            .iter()
            .find(|dataset| dataset.id() == id)
            .cloned())
    }
}

fn official_validated(datasets: &[RiskDataset]) -> u64 {
    datasets
        .iter()
        .filter(|dataset| dataset.origin() == DatasetOrigin::Official && dataset.is_usable())
        .count() as u64
}

#[derive(Clone)]
pub enum AppStore {
    Memory(MemoryStore),
    Postgres(crate::postgres::PgStore),
    Unavailable { reason: String },
}

impl DatasetReadModel for AppStore {
    async fn ping(&self) -> Result<(), DataError> {
        match self {
            Self::Memory(store) => store.ping().await,
            Self::Postgres(store) => store.ping().await,
            Self::Unavailable { reason } => Err(DataError::Database {
                detail: reason.clone(),
            }),
        }
    }

    async fn validated_dataset_count(&self) -> Result<u64, DataError> {
        match self {
            Self::Memory(store) => store.validated_dataset_count().await,
            Self::Postgres(store) => store.validated_dataset_count().await,
            Self::Unavailable { reason } => Err(DataError::Database {
                detail: reason.clone(),
            }),
        }
    }

    async fn list_datasets(&self) -> Result<DatasetPage, DataError> {
        match self {
            Self::Memory(store) => store.list_datasets().await,
            Self::Postgres(store) => store.list_datasets().await,
            Self::Unavailable { reason } => Err(DataError::Database {
                detail: reason.clone(),
            }),
        }
    }

    async fn get_dataset(&self, id: DatasetId) -> Result<Option<RiskDataset>, DataError> {
        match self {
            Self::Memory(store) => store.get_dataset(id).await,
            Self::Postgres(store) => store.get_dataset(id).await,
            Self::Unavailable { reason } => Err(DataError::Database {
                detail: reason.clone(),
            }),
        }
    }
}

impl AppStore {
    pub async fn span_margin(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_datetime: chrono::NaiveDateTime,
        positions: &[MarginPosition],
    ) -> Result<crate::StoredSpanQuote, DataError> {
        match self {
            Self::Postgres(store) => {
                store
                    .span_margin(exchange, clearing, trading_datetime, positions)
                    .await
            }
            Self::Memory(_) => Err(DataError::MarginRejected {
                detail: "the memory store has no archived SPAN bytes".to_owned(),
            }),
            Self::Unavailable { reason } => Err(DataError::Database {
                detail: reason.clone(),
            }),
        }
    }
}
