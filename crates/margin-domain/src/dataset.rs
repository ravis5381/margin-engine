use crate::clearing::{ClearingCorporation, Exchange};
use crate::error::DomainError;
use crate::ids::{ContractMasterVersionId, DatasetId, Sha256Hex};
use chrono::{DateTime, NaiveDate, Utc};

/// Whether the bytes came from an official source or from a marked synthetic fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DatasetOrigin {
    Synthetic,
    Official,
}

impl DatasetOrigin {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic",
            Self::Official => "official",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "synthetic" => Ok(Self::Synthetic),
            "official" => Ok(Self::Official),
            other => Err(DomainError::DatasetNotUsable {
                detail: format!("unknown dataset origin `{other}`"),
            }),
        }
    }
}

impl std::fmt::Display for DatasetOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnapshotClass {
    BeginOfDay,
    Intraday,
    EndOfDay,
}

impl SnapshotClass {
    pub const fn code(self) -> &'static str {
        match self {
            Self::BeginOfDay => "begin_of_day",
            Self::Intraday => "intraday",
            Self::EndOfDay => "end_of_day",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "begin_of_day" => Ok(Self::BeginOfDay),
            "intraday" => Ok(Self::Intraday),
            "end_of_day" => Ok(Self::EndOfDay),
            other => Err(DomainError::DatasetNotUsable {
                detail: format!("unknown snapshot class `{other}`"),
            }),
        }
    }
}

impl std::fmt::Display for SnapshotClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotLocator {
    class: SnapshotClass,
    intraday_sequence: Option<u32>,
}

impl SnapshotLocator {
    pub fn begin_of_day() -> Self {
        Self {
            class: SnapshotClass::BeginOfDay,
            intraday_sequence: None,
        }
    }

    pub fn end_of_day() -> Self {
        Self {
            class: SnapshotClass::EndOfDay,
            intraday_sequence: None,
        }
    }

    pub fn intraday(sequence: u32) -> Self {
        Self {
            class: SnapshotClass::Intraday,
            intraday_sequence: Some(sequence),
        }
    }

    pub fn try_new(
        class: SnapshotClass,
        intraday_sequence: Option<u32>,
    ) -> Result<Self, DomainError> {
        match (class, intraday_sequence) {
            (SnapshotClass::Intraday, Some(sequence)) => Ok(Self {
                class,
                intraday_sequence: Some(sequence),
            }),
            (SnapshotClass::Intraday, None) => Err(DomainError::DatasetNotUsable {
                detail: "intraday snapshot requires a sequence number".to_owned(),
            }),
            (class, None) => Ok(Self {
                class,
                intraday_sequence: None,
            }),
            (_, Some(_)) => Err(DomainError::DatasetNotUsable {
                detail: "only an intraday snapshot may carry a sequence number".to_owned(),
            }),
        }
    }

    pub fn class(self) -> SnapshotClass {
        self.class
    }

    pub fn intraday_sequence(self) -> Option<u32> {
        self.intraday_sequence
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationStatus {
    Pending,
    Passed,
    Failed,
}

impl ValidationStatus {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "pending" => Ok(Self::Pending),
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            other => Err(DomainError::DatasetNotUsable {
                detail: format!("unknown validation status `{other}`"),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicationStatus {
    Draft,
    Published,
    Superseded { superseded_by: DatasetId },
    Rejected,
}

impl PublicationStatus {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Superseded { .. } => "superseded",
            Self::Rejected => "rejected",
        }
    }

    pub fn superseded_by(&self) -> Option<DatasetId> {
        match self {
            Self::Superseded { superseded_by } => Some(*superseded_by),
            _ => None,
        }
    }

    pub fn from_db(code: &str, superseded_by: Option<DatasetId>) -> Result<Self, DomainError> {
        match (code, superseded_by) {
            ("draft", None) => Ok(Self::Draft),
            ("published", None) => Ok(Self::Published),
            ("rejected", None) => Ok(Self::Rejected),
            ("superseded", Some(dataset_id)) => Ok(Self::Superseded {
                superseded_by: dataset_id,
            }),
            _ => Err(DomainError::DatasetNotUsable {
                detail: format!("publication status `{code}` does not match superseded_by"),
            }),
        }
    }
}

/// Immutable description of one archived risk file and its validation state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskDataset {
    id: DatasetId,
    exchange: Exchange,
    clearing_corporation: ClearingCorporation,
    trading_date: NaiveDate,
    snapshot: SnapshotLocator,
    publication_timestamp: Option<DateTime<Utc>>,
    download_timestamp: DateTime<Utc>,
    original_filename: String,
    source_host: Option<String>,
    sha256: Sha256Hex,
    file_format: String,
    parser_version: String,
    contract_master_version_id: Option<ContractMasterVersionId>,
    validation_status: ValidationStatus,
    validation_error_summary: Option<String>,
    publication_status: PublicationStatus,
    origin: DatasetOrigin,
}

pub struct RiskDatasetDraft {
    pub id: DatasetId,
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub trading_date: NaiveDate,
    pub snapshot: SnapshotLocator,
    pub publication_timestamp: Option<DateTime<Utc>>,
    pub download_timestamp: DateTime<Utc>,
    pub original_filename: String,
    pub source_host: Option<String>,
    pub sha256: Sha256Hex,
    pub file_format: String,
    pub parser_version: String,
    pub contract_master_version_id: Option<ContractMasterVersionId>,
    pub validation_status: ValidationStatus,
    pub validation_error_summary: Option<String>,
    pub publication_status: PublicationStatus,
    pub origin: DatasetOrigin,
}

impl RiskDataset {
    pub fn try_new(draft: RiskDatasetDraft) -> Result<Self, DomainError> {
        validate_filename(&draft.original_filename)?;
        validate_label(&draft.file_format, "file format")?;
        validate_label(&draft.parser_version, "parser version")?;
        match draft.origin {
            DatasetOrigin::Synthetic if draft.source_host.is_some() => {
                return Err(DomainError::DatasetNotUsable {
                    detail: "synthetic datasets cannot carry an official source host".to_owned(),
                });
            }
            DatasetOrigin::Official => match draft.source_host.as_deref() {
                Some(host) if !host.trim().is_empty() && !host.contains('/') => {}
                _ => {
                    return Err(DomainError::DatasetNotUsable {
                        detail: "official datasets require a source host".to_owned(),
                    });
                }
            },
            DatasetOrigin::Synthetic => {}
        }
        let summary_ok = match (
            &draft.validation_status,
            draft.validation_error_summary.as_deref(),
        ) {
            (ValidationStatus::Failed, Some(summary)) if !summary.trim().is_empty() => true,
            (ValidationStatus::Failed, _) => false,
            (_, None) => true,
            _ => false,
        };
        if !summary_ok {
            return Err(DomainError::DatasetNotUsable {
                detail: "failed validation requires a summary, and other states forbid one"
                    .to_owned(),
            });
        }
        let lifecycle_ok = matches!(
            (
                &draft.publication_status,
                draft.validation_status,
                draft.contract_master_version_id,
            ),
            (PublicationStatus::Draft, ValidationStatus::Pending, _)
                | (PublicationStatus::Rejected, ValidationStatus::Failed, _)
                | (
                    PublicationStatus::Published,
                    ValidationStatus::Passed,
                    Some(_)
                )
                | (
                    PublicationStatus::Superseded { .. },
                    ValidationStatus::Passed,
                    Some(_)
                )
        );
        if !lifecycle_ok {
            return Err(DomainError::DatasetNotUsable {
                detail: "dataset publication and validation states are inconsistent".to_owned(),
            });
        }
        Ok(Self {
            id: draft.id,
            exchange: draft.exchange,
            clearing_corporation: draft.clearing_corporation,
            trading_date: draft.trading_date,
            snapshot: draft.snapshot,
            publication_timestamp: draft.publication_timestamp,
            download_timestamp: draft.download_timestamp,
            original_filename: draft.original_filename,
            source_host: draft.source_host,
            sha256: draft.sha256,
            file_format: draft.file_format,
            parser_version: draft.parser_version,
            contract_master_version_id: draft.contract_master_version_id,
            validation_status: draft.validation_status,
            validation_error_summary: draft.validation_error_summary,
            publication_status: draft.publication_status,
            origin: draft.origin,
        })
    }

    pub fn id(&self) -> DatasetId {
        self.id
    }

    pub fn exchange(&self) -> Exchange {
        self.exchange
    }

    pub fn clearing_corporation(&self) -> ClearingCorporation {
        self.clearing_corporation
    }

    pub fn trading_date(&self) -> NaiveDate {
        self.trading_date
    }

    pub fn snapshot(&self) -> SnapshotLocator {
        self.snapshot
    }

    pub fn publication_timestamp(&self) -> Option<DateTime<Utc>> {
        self.publication_timestamp
    }

    pub fn download_timestamp(&self) -> DateTime<Utc> {
        self.download_timestamp
    }

    pub fn original_filename(&self) -> &str {
        &self.original_filename
    }

    pub fn source_host(&self) -> Option<&str> {
        self.source_host.as_deref()
    }

    pub fn sha256(&self) -> &Sha256Hex {
        &self.sha256
    }

    pub fn file_format(&self) -> &str {
        &self.file_format
    }

    pub fn parser_version(&self) -> &str {
        &self.parser_version
    }

    pub fn contract_master_version_id(&self) -> Option<ContractMasterVersionId> {
        self.contract_master_version_id
    }

    pub fn validation_status(&self) -> ValidationStatus {
        self.validation_status
    }

    pub fn validation_error_summary(&self) -> Option<&str> {
        self.validation_error_summary.as_deref()
    }

    pub fn publication_status(&self) -> &PublicationStatus {
        &self.publication_status
    }

    pub fn origin(&self) -> DatasetOrigin {
        self.origin
    }

    pub fn is_usable(&self) -> bool {
        self.validation_status == ValidationStatus::Passed
            && matches!(self.publication_status, PublicationStatus::Published)
    }

    pub fn require_official(&self) -> Result<(), DomainError> {
        match self.origin {
            DatasetOrigin::Official => Ok(()),
            DatasetOrigin::Synthetic => Err(DomainError::DatasetNotUsable {
                detail: "synthetic datasets cannot be used for an official margin calculation"
                    .to_owned(),
            }),
        }
    }
}

fn validate_filename(filename: &str) -> Result<(), DomainError> {
    if filename.is_empty()
        || filename.len() > 255
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
    {
        return Err(DomainError::DatasetNotUsable {
            detail: "original filename is empty, too long, or contains a path".to_owned(),
        });
    }
    Ok(())
}

fn validate_label(value: &str, what: &str) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.len() > 64 {
        return Err(DomainError::DatasetNotUsable {
            detail: format!("{what} must be 1 to 64 characters"),
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatasetSelector {
    /// Latest ingested snapshot for the exact trade date and snapshot class.
    ///
    /// A newer ingested snapshot that is not published and passed blocks older snapshots.
    /// Datasets from other dates are never substituted.
    LatestValidated,
    Exact(DatasetId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatasetQuery {
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub trade_date: NaiveDate,
    pub snapshot_class: SnapshotClass,
    pub selector: DatasetSelector,
}

pub fn select_dataset<'a>(
    datasets: &'a [RiskDataset],
    query: &DatasetQuery,
) -> Result<&'a RiskDataset, DomainError> {
    match query.selector {
        DatasetSelector::Exact(id) => select_exact(datasets, query, id),
        DatasetSelector::LatestValidated => select_latest(datasets, query),
    }
}

fn select_exact<'a>(
    datasets: &'a [RiskDataset],
    query: &DatasetQuery,
    id: DatasetId,
) -> Result<&'a RiskDataset, DomainError> {
    let Some(dataset) = datasets.iter().find(|dataset| dataset.id == id) else {
        return Err(DomainError::MissingRequiredData {
            detail: format!("risk dataset {id} was not found"),
        });
    };
    if dataset.exchange != query.exchange
        || dataset.clearing_corporation != query.clearing_corporation
        || dataset.trading_date != query.trade_date
        || dataset.snapshot.class != query.snapshot_class
    {
        return Err(DomainError::StaleRiskData {
            detail: format!(
                "risk dataset {id} does not match the requested exchange, clearing corporation, trade date, and snapshot"
            ),
        });
    }
    if !dataset.is_usable() {
        return Err(DomainError::DatasetNotUsable {
            detail: format!("risk dataset {id} is not a published validated dataset"),
        });
    }
    Ok(dataset)
}

fn select_latest<'a>(
    datasets: &'a [RiskDataset],
    query: &DatasetQuery,
) -> Result<&'a RiskDataset, DomainError> {
    let scoped: Vec<&RiskDataset> = datasets
        .iter()
        .filter(|dataset| {
            dataset.exchange == query.exchange
                && dataset.clearing_corporation == query.clearing_corporation
                && dataset.snapshot.class == query.snapshot_class
        })
        .collect();
    let same_day: Vec<&RiskDataset> = scoped
        .iter()
        .copied()
        .filter(|dataset| dataset.trading_date == query.trade_date)
        .collect();
    if same_day.is_empty() {
        if scoped.is_empty() {
            return Err(DomainError::MissingRequiredData {
                detail: "no risk dataset is available for the requested exchange, clearing corporation, and snapshot".to_owned(),
            });
        }
        return Err(DomainError::StaleRiskData {
            detail: "no risk dataset exists for the requested trade date; other dates were not substituted".to_owned(),
        });
    }
    let with_publication = same_day
        .iter()
        .filter(|dataset| dataset.publication_timestamp.is_some())
        .count();
    if with_publication != 0 && with_publication != same_day.len() {
        return Err(DomainError::AmbiguousDataset {
            detail: "candidate datasets disagree on whether a publication timestamp exists"
                .to_owned(),
        });
    }
    let use_publication = with_publication == same_day.len();
    let max_sequence = same_day
        .iter()
        .map(|dataset| dataset.snapshot.intraday_sequence.unwrap_or(0))
        .max()
        .ok_or_else(|| DomainError::MissingRequiredData {
            detail: "candidate dataset set was empty".to_owned(),
        })?;
    let at_sequence: Vec<&RiskDataset> = same_day
        .into_iter()
        .filter(|dataset| dataset.snapshot.intraday_sequence.unwrap_or(0) == max_sequence)
        .collect();
    let max_time = at_sequence
        .iter()
        .map(|dataset| sort_time(dataset, use_publication))
        .max()
        .ok_or_else(|| DomainError::MissingRequiredData {
            detail: "candidate dataset set was empty".to_owned(),
        })?;
    let mut top: Vec<&RiskDataset> = at_sequence
        .into_iter()
        .filter(|dataset| sort_time(dataset, use_publication) == max_time)
        .collect();
    top.sort_by_key(|dataset| dataset.id);
    let distinct_hashes = {
        let mut hashes: Vec<&Sha256Hex> = top.iter().map(|dataset| &dataset.sha256).collect();
        hashes.sort();
        hashes.dedup();
        hashes.len()
    };
    if distinct_hashes != 1 {
        return Err(DomainError::AmbiguousDataset {
            detail: "the newest snapshot rank contains more than one file hash".to_owned(),
        });
    }
    let chosen = top
        .first()
        .copied()
        .ok_or_else(|| DomainError::MissingRequiredData {
            detail: "candidate dataset set was empty".to_owned(),
        })?;
    if !chosen.is_usable() {
        return Err(DomainError::NewerDatasetNotUsable {
            detail: format!(
                "dataset {} has publication {} and validation {}",
                chosen.id,
                chosen.publication_status.code(),
                chosen.validation_status.code()
            ),
        });
    }
    Ok(chosen)
}

fn sort_time(dataset: &RiskDataset, use_publication: bool) -> DateTime<Utc> {
    if use_publication {
        dataset
            .publication_timestamp
            .unwrap_or(dataset.download_timestamp)
    } else {
        dataset.download_timestamp
    }
}

/// Same archived bytes must not create another dataset version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestDisposition<'a> {
    Duplicate { existing: &'a RiskDataset },
    New,
}

pub fn classify_ingest<'a>(
    existing: &'a [RiskDataset],
    sha256: &Sha256Hex,
) -> IngestDisposition<'a> {
    match existing.iter().find(|dataset| dataset.sha256() == sha256) {
        Some(existing) => IngestDisposition::Duplicate { existing },
        None => IngestDisposition::New,
    }
}
