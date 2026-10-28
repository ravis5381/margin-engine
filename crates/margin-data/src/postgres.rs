use crate::DataError;
use crate::span_quote::{
    ContractElmRates, ExposureRates, MarginPosition, SpanMarginQuote, quote_span_zip,
};
use crate::store::{DatasetPage, DatasetReadModel, summarize};
use chrono::{DateTime, NaiveDate, Utc};
use margin_domain::{
    ClearingCorporation, ContractMasterVersionId, DatasetId, DatasetOrigin, Exchange,
    PublicationStatus, RiskDataset, RiskDatasetDraft, Sha256Hex, SnapshotClass, SnapshotLocator,
    ValidationStatus,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

pub async fn connect(
    url: &str,
    max_connections: u32,
    acquire_timeout: Duration,
) -> Result<PgPool, DataError> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(acquire_timeout)
        .connect(url)
        .await
        .map_err(|error| {
            tracing::error!(
                error = %crate::redact::redact(&error.to_string(), &[url]),
                "database connection failed"
            );
            DataError::Database {
                detail: "database connection failed".to_owned(),
            }
        })
}

pub async fn migrate(pool: &PgPool) -> Result<(), DataError> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "database migration failed");
            DataError::Database {
                detail: "database migration failed".to_owned(),
            }
        })
}

impl DatasetReadModel for PgStore {
    async fn ping(&self) -> Result<(), DataError> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map(|_| ())
            .map_err(db_error_message)
    }

    async fn validated_dataset_count(&self) -> Result<u64, DataError> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM risk_datasets
            WHERE publication_status = 'published'
              AND validation_status = 'passed'
              AND origin = 'official'
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db_error_message)?;
        u64::try_from(count).map_err(|_| DataError::Database {
            detail: "validated dataset count is negative".to_owned(),
        })
    }

    async fn list_datasets(&self) -> Result<DatasetPage, DataError> {
        let rows = sqlx::query_as::<_, DatasetRow>(&select_sql("LIMIT $1"))
            .bind(501_i64)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error_message)?;
        let truncated = rows.len() > 500;
        let mut datasets = Vec::new();
        for row in rows.into_iter().take(500) {
            datasets.push(summarize(&dataset_from_row(row)?));
        }
        Ok(DatasetPage {
            datasets,
            truncated,
        })
    }

    async fn get_dataset(&self, id: DatasetId) -> Result<Option<RiskDataset>, DataError> {
        let row = sqlx::query_as::<_, DatasetRow>(&format!("{} WHERE id = $1", select_columns()))
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error_message)?;
        row.map(dataset_from_row).transpose()
    }
}

#[derive(Debug, sqlx::FromRow)]
struct SpanCandidate {
    id: Uuid,
    snapshot_class: String,
    intraday_sequence: Option<i32>,
    original_filename: String,
    sha256: String,
    publication_status: String,
    validation_status: String,
}

pub struct StoredSpanQuote {
    pub dataset_id: Uuid,
    pub trading_date: NaiveDate,
    pub snapshot_class: String,
    pub intraday_sequence: Option<u32>,
    pub filename: String,
    pub sha256: String,
    pub quote: SpanMarginQuote,
}

impl PgStore {
    pub async fn span_margin(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
        positions: &[MarginPosition],
    ) -> Result<StoredSpanQuote, DataError> {
        let candidates = sqlx::query_as::<_, SpanCandidate>(
            "SELECT id, snapshot_class, intraday_sequence, original_filename, sha256, \
             publication_status, validation_status \
             FROM risk_datasets \
             WHERE exchange = $1 AND clearing_corporation = $2 AND trading_date = $3 \
               AND origin = 'official' AND file_format = 'nse-span-xml-4.00-zip' \
               AND parser_version = 'span-xml-4.00'",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error_message)?;
        let chosen = choose_span_candidate(&candidates, trading_date)?;
        let bytes: Vec<u8> = sqlx::query_scalar("SELECT content FROM risk_files WHERE sha256 = $1")
            .bind(&chosen.sha256)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error_message)?;
        let exposure_bytes = self.exposure_file(exchange, clearing, trading_date).await?;
        let exposure = ExposureRates::parse_csv(&exposure_bytes, trading_date)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        let contract_bytes = self
            .contract_elm_file(exchange, clearing, trading_date)
            .await?;
        let contracts = ContractElmRates::parse_gzip(&contract_bytes)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        let quote = quote_span_zip(&bytes, trading_date, positions, &exposure, &contracts)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        Ok(StoredSpanQuote {
            dataset_id: chosen.id,
            trading_date,
            snapshot_class: chosen.snapshot_class.clone(),
            intraday_sequence: chosen
                .intraday_sequence
                .map(|sequence| u32::try_from(sequence).unwrap_or(u32::MAX)),
            filename: chosen.original_filename.clone(),
            sha256: chosen.sha256.clone(),
            quote,
        })
    }
}

impl PgStore {
    async fn exposure_file(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<Vec<u8>, DataError> {
        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT sha256, content FROM official_auxiliary_files \
             WHERE exchange = $1 AND clearing_corporation = $2 AND trading_date = $3 \
               AND kind = 'exposure_limit_csv' AND check_status = 'container_checked'",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error_message)?;
        if rows.is_empty() {
            return Err(DataError::MarginRejected {
                detail: format!(
                    "no exposure file is archived for {trading_date}, so extreme loss margin is not calculated"
                ),
            });
        }
        let mut hashes: Vec<&str> = rows.iter().map(|(sha, _)| sha.as_str()).collect();
        hashes.sort_unstable();
        hashes.dedup();
        if hashes.len() != 1 {
            return Err(DataError::MarginRejected {
                detail: format!("more than one exposure file hash is archived for {trading_date}"),
            });
        }
        Ok(rows[0].1.clone())
    }

    async fn contract_elm_file(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<Vec<u8>, DataError> {
        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT sha256, content FROM official_auxiliary_files \
             WHERE exchange = $1 AND clearing_corporation = $2 AND trading_date = $3 \
               AND kind = 'contract_elm_gzip' AND check_status = 'container_checked'",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error_message)?;
        if rows.is_empty() {
            return Err(DataError::MarginRejected {
                detail: format!(
                    "no contract ELM file is archived for {trading_date}, so extreme loss margin is not calculated"
                ),
            });
        }
        let mut hashes: Vec<&str> = rows.iter().map(|(sha, _)| sha.as_str()).collect();
        hashes.sort_unstable();
        hashes.dedup();
        if hashes.len() != 1 {
            return Err(DataError::MarginRejected {
                detail: format!(
                    "more than one contract ELM file hash is archived for {trading_date}"
                ),
            });
        }
        Ok(rows[0].1.clone())
    }
}

fn choose_span_candidate(
    candidates: &[SpanCandidate],
    trading_date: NaiveDate,
) -> Result<&SpanCandidate, DataError> {
    if candidates.is_empty() {
        return Err(DataError::MarginNotFound {
            detail: format!("no official SPAN file is archived for {trading_date}"),
        });
    }
    let mut ranked = Vec::new();
    for candidate in candidates {
        let rank = snapshot_rank(&candidate.snapshot_class, candidate.intraday_sequence)?;
        ranked.push((rank, candidate));
    }
    let top_rank = ranked.iter().map(|(rank, _)| *rank).max().unwrap_or(0);
    let top: Vec<&SpanCandidate> = ranked
        .into_iter()
        .filter(|(rank, _)| *rank == top_rank)
        .map(|(_, candidate)| candidate)
        .collect();
    let mut hashes: Vec<&str> = top
        .iter()
        .map(|candidate| candidate.sha256.as_str())
        .collect();
    hashes.sort_unstable();
    hashes.dedup();
    if hashes.len() != 1 {
        return Err(DataError::MarginRejected {
            detail: "the newest SPAN snapshot rank contains more than one file hash".to_owned(),
        });
    }
    let chosen = top[0];
    if chosen.publication_status == "rejected" || chosen.validation_status == "failed" {
        return Err(DataError::MarginRejected {
            detail: format!(
                "the newest SPAN snapshot {} is {} and {}; an older snapshot was not substituted",
                chosen.original_filename, chosen.publication_status, chosen.validation_status
            ),
        });
    }
    if chosen.publication_status != "draft" && chosen.publication_status != "published" {
        return Err(DataError::MarginRejected {
            detail: format!(
                "the newest SPAN snapshot {} has publication status {}",
                chosen.original_filename, chosen.publication_status
            ),
        });
    }
    Ok(chosen)
}

fn snapshot_rank(class: &str, sequence: Option<i32>) -> Result<i32, DataError> {
    match (class, sequence) {
        ("begin_of_day", None) => Ok(0),
        ("intraday", Some(sequence)) if (1..=4).contains(&sequence) => Ok(sequence),
        ("end_of_day", None) => Ok(5),
        _ => Err(DataError::MarginRejected {
            detail: format!("SPAN snapshot {class} cannot be ranked"),
        }),
    }
}

pub(crate) fn select_columns() -> &'static str {
    "SELECT id, exchange, clearing_corporation, trading_date, snapshot_class, \
     intraday_sequence, publication_timestamp, download_timestamp, original_filename, \
     source_host, sha256, file_format, parser_version, contract_master_version_id, \
     validation_status, validation_error_summary, publication_status, superseded_by, origin \
     FROM risk_datasets"
}

fn select_sql(suffix: &str) -> String {
    format!(
        "{} ORDER BY trading_date DESC, download_timestamp DESC {suffix}",
        select_columns()
    )
}

pub(crate) fn db_error_message(error: sqlx::Error) -> DataError {
    tracing::error!(error = %error, "database query failed");
    DataError::Database {
        detail: "database query failed".to_owned(),
    }
}

pub(crate) fn dataset_from_row(row: DatasetRow) -> Result<RiskDataset, DataError> {
    let exchange = Exchange::parse(&row.exchange)?;
    let clearing_corporation = ClearingCorporation::parse(&row.clearing_corporation)?;
    let class = SnapshotClass::parse(&row.snapshot_class)?;
    let sequence = match row.intraday_sequence {
        Some(value) if value < 0 => {
            return Err(DataError::Database {
                detail: "intraday sequence is negative".to_owned(),
            });
        }
        Some(value) => Some(u32::try_from(value).map_err(|_| DataError::Database {
            detail: "intraday sequence does not fit".to_owned(),
        })?),
        None => None,
    };
    let snapshot = SnapshotLocator::try_new(class, sequence)?;
    let publication_status = PublicationStatus::from_db(
        &row.publication_status,
        row.superseded_by.map(DatasetId::from_uuid),
    )?;
    Ok(RiskDataset::try_new(RiskDatasetDraft {
        id: DatasetId::from_uuid(row.id),
        exchange,
        clearing_corporation,
        trading_date: row.trading_date,
        snapshot,
        publication_timestamp: row.publication_timestamp,
        download_timestamp: row.download_timestamp,
        original_filename: row.original_filename,
        source_host: row.source_host,
        sha256: Sha256Hex::parse(&row.sha256)?,
        file_format: row.file_format,
        parser_version: row.parser_version,
        contract_master_version_id: row
            .contract_master_version_id
            .map(ContractMasterVersionId::from_uuid),
        validation_status: ValidationStatus::parse(&row.validation_status)?,
        validation_error_summary: row.validation_error_summary,
        publication_status,
        origin: DatasetOrigin::parse(&row.origin)?,
    })?)
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct DatasetRow {
    pub id: Uuid,
    pub exchange: String,
    pub clearing_corporation: String,
    pub trading_date: NaiveDate,
    pub snapshot_class: String,
    pub intraday_sequence: Option<i32>,
    pub publication_timestamp: Option<DateTime<Utc>>,
    pub download_timestamp: DateTime<Utc>,
    pub original_filename: String,
    pub source_host: Option<String>,
    pub sha256: String,
    pub file_format: String,
    pub parser_version: String,
    pub contract_master_version_id: Option<Uuid>,
    pub validation_status: String,
    pub validation_error_summary: Option<String>,
    pub publication_status: String,
    pub superseded_by: Option<Uuid>,
    pub origin: String,
}

#[cfg(test)]
mod tests {
    use super::{DatasetRow, dataset_from_row};
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn sample_row() -> DatasetRow {
        DatasetRow {
            id: Uuid::nil(),
            exchange: "NSE".to_owned(),
            clearing_corporation: "NSCCL".to_owned(),
            trading_date: chrono::NaiveDate::from_ymd_opt(2026, 10, 28).expect("date"),
            snapshot_class: "end_of_day".to_owned(),
            intraday_sequence: None,
            publication_timestamp: Some(Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap()),
            download_timestamp: Utc.with_ymd_and_hms(2026, 10, 28, 12, 5, 0).unwrap(),
            original_filename: "synthetic.dat".to_owned(),
            source_host: None,
            sha256: "ab".repeat(32),
            file_format: "synthetic".to_owned(),
            parser_version: "none".to_owned(),
            contract_master_version_id: Some(Uuid::from_u128(7)),
            validation_status: "passed".to_owned(),
            validation_error_summary: None,
            publication_status: "published".to_owned(),
            superseded_by: None,
            origin: "synthetic".to_owned(),
        }
    }

    #[test]
    fn published_synthetic_row_maps_and_inconsistent_row_fails() {
        let dataset = dataset_from_row(sample_row()).expect("row");
        assert!(dataset.is_usable());
        assert!(dataset.require_official().is_err());
        let mut broken = sample_row();
        broken.validation_status = "failed".to_owned();
        broken.validation_error_summary = Some("schema mismatch".to_owned());
        assert!(dataset_from_row(broken).is_err());
    }
}
