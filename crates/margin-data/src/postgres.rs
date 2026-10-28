use crate::DataError;
use crate::nse::reports::{NseReportKind, SPAN_FILE_FORMAT};
use crate::span::quote::{MarginPosition, SpanMarginQuote};
use crate::span::xml::{PARSER_VERSION, span_created_timestamp};
use crate::store::{DatasetPage, DatasetReadModel, summarize};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
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
pub(crate) struct SpanCandidate {
    id: Uuid,
    snapshot_class: String,
    intraday_sequence: Option<i32>,
    original_filename: String,
    sha256: String,
    publication_status: String,
    validation_status: String,
    prefix: Vec<u8>,
}

pub(crate) struct ChosenSpan<'a> {
    pub chosen: &'a SpanCandidate,
    pub span_created: NaiveDateTime,
    pub trading_date: NaiveDate,
    pub trading_datetime: NaiveDateTime,
    pub bytes: &'a [u8],
}

pub struct StoredSpanQuote {
    pub dataset_id: Uuid,
    pub trading_date: NaiveDate,
    pub trading_datetime: NaiveDateTime,
    pub span_created: NaiveDateTime,
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
        trading_datetime: NaiveDateTime,
        positions: &[MarginPosition],
    ) -> Result<StoredSpanQuote, DataError> {
        let trading_date = trading_datetime.date();
        let candidates = sqlx::query_as::<_, SpanCandidate>(
            "SELECT d.id, d.snapshot_class, d.intraday_sequence, d.original_filename, d.sha256, \
             d.publication_status, d.validation_status, \
             substring(f.content from 1 for 80000) AS prefix \
             FROM risk_datasets d \
             JOIN risk_files f ON f.sha256 = d.sha256 \
             WHERE d.exchange = $1 AND d.clearing_corporation = $2 AND d.trading_date = $3 \
               AND d.origin = 'official' AND d.file_format = $4 \
               AND d.parser_version = 'span-xml-4.00'",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .bind(span_file_format(exchange))
        .fetch_all(&self.pool)
        .await
        .map_err(db_error_message)?;
        let (chosen, span_created) = choose_span_as_of(&candidates, trading_datetime)?;
        let bytes: Vec<u8> = sqlx::query_scalar("SELECT content FROM risk_files WHERE sha256 = $1")
            .bind(&chosen.sha256)
            .fetch_one(&self.pool)
            .await
            .map_err(db_error_message)?;
        let selected = ChosenSpan {
            chosen,
            span_created,
            trading_date,
            trading_datetime,
            bytes: &bytes,
        };
        match exchange {
            Exchange::Nse => self.quote_nse(clearing, &selected, positions).await,
            Exchange::Bse => self.quote_iccl(&selected, positions).await,
        }
    }
}

pub(crate) fn finish_span_quote(
    chosen: &SpanCandidate,
    trading_date: NaiveDate,
    trading_datetime: NaiveDateTime,
    span_created: NaiveDateTime,
    quote: SpanMarginQuote,
) -> StoredSpanQuote {
    StoredSpanQuote {
        dataset_id: chosen.id,
        trading_date,
        trading_datetime,
        span_created,
        snapshot_class: chosen.snapshot_class.clone(),
        intraday_sequence: chosen
            .intraday_sequence
            .map(|sequence| u32::try_from(sequence).unwrap_or(u32::MAX)),
        filename: chosen.original_filename.clone(),
        sha256: chosen.sha256.clone(),
        quote,
    }
}

pub(crate) fn one_auxiliary_hash(
    rows: Vec<(String, Vec<u8>)>,
    duplicate: String,
) -> Result<Vec<u8>, DataError> {
    let mut hashes: Vec<&str> = rows.iter().map(|(sha, _)| sha.as_str()).collect();
    hashes.sort_unstable();
    hashes.dedup();
    if hashes.len() != 1 {
        return Err(DataError::MarginRejected { detail: duplicate });
    }
    Ok(rows[0].1.clone())
}

fn span_file_format(exchange: Exchange) -> &'static str {
    match exchange {
        Exchange::Nse => SPAN_FILE_FORMAT,
        Exchange::Bse => crate::iccl::ingest::ICCL_SPAN_FILE_FORMAT,
    }
}

impl PgStore {
    pub(crate) async fn auxiliary_rows(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
        kind: &str,
        check_status: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, DataError> {
        sqlx::query_as(
            "SELECT sha256, content FROM official_auxiliary_files \
             WHERE exchange = $1 AND clearing_corporation = $2 AND trading_date = $3 \
               AND kind = $4 AND check_status = $5",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .bind(kind)
        .bind(check_status)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error_message)
    }

    pub(crate) async fn span_snapshot_archived(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
        file_format: &str,
        snapshot: &margin_domain::SnapshotLocator,
    ) -> Result<bool, DataError> {
        let sequence = snapshot
            .intraday_sequence()
            .map(|value| i32::try_from(value).unwrap_or(i32::MAX));
        let present = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM risk_datasets
                WHERE exchange = $1 AND clearing_corporation = $2 AND trading_date = $3
                  AND origin = 'official' AND file_format = $4 AND parser_version = $5
                  AND snapshot_class = $6
                  AND intraday_sequence IS NOT DISTINCT FROM $7
                  AND publication_status IN ('draft', 'published')
                  AND validation_status <> 'failed'
             )",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .bind(file_format)
        .bind(PARSER_VERSION)
        .bind(snapshot.class().code())
        .bind(sequence)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error_message)?;
        Ok(present)
    }

    pub(crate) async fn exposure_file(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<Vec<u8>, DataError> {
        self.one_auxiliary(
            exchange,
            clearing,
            trading_date,
            "exposure_limit_csv",
            format!(
                "no exposure file is archived for {trading_date}, so extreme loss margin is not calculated"
            ),
            format!("more than one exposure file hash is archived for {trading_date}"),
        )
        .await
    }

    pub(crate) async fn contract_elm_file(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<Vec<u8>, DataError> {
        self.one_auxiliary(
            exchange,
            clearing,
            trading_date,
            "contract_elm_gzip",
            format!(
                "no contract ELM file is archived for {trading_date}, so extreme loss margin is not calculated"
            ),
            format!("more than one contract ELM file hash is archived for {trading_date}"),
        )
        .await
    }

    pub(crate) async fn contract_master_file(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<Vec<u8>, DataError> {
        self.one_auxiliary(
            exchange,
            clearing,
            trading_date,
            "contract_master_gzip",
            format!("no contract master is archived for {trading_date}"),
            format!("more than one contract master hash is archived for {trading_date}"),
        )
        .await
    }

    pub(crate) async fn cash_var_file(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<Option<Vec<u8>>, DataError> {
        let rows = self
            .auxiliary_rows(
                exchange,
                clearing,
                trading_date,
                "cash_var_dat",
                "container_checked",
            )
            .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        one_auxiliary_hash(
            rows,
            format!("more than one cash-market VaR file hash is archived for {trading_date}"),
        )
        .map(Some)
    }

    async fn one_auxiliary(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
        kind: &str,
        missing: String,
        duplicate: String,
    ) -> Result<Vec<u8>, DataError> {
        let rows = self
            .auxiliary_rows(exchange, clearing, trading_date, kind, "container_checked")
            .await?;
        if rows.is_empty() {
            return Err(DataError::MarginRejected { detail: missing });
        }
        one_auxiliary_hash(rows, duplicate)
    }

    pub(crate) async fn report_is_archived(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
        kind: NseReportKind,
    ) -> Result<bool, DataError> {
        if let Some(auxiliary_kind) = kind.auxiliary_kind() {
            return self
                .auxiliary_is_archived(exchange, clearing, trading_date, auxiliary_kind)
                .await;
        }
        let Some(snapshot) = kind.snapshot() else {
            return Ok(false);
        };
        self.span_snapshot_archived(
            exchange,
            clearing,
            trading_date,
            SPAN_FILE_FORMAT,
            &snapshot,
        )
        .await
    }

    pub(crate) async fn auxiliary_is_archived(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
        kind: &str,
    ) -> Result<bool, DataError> {
        let present = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM official_auxiliary_files
                WHERE exchange = $1 AND clearing_corporation = $2 AND trading_date = $3
                  AND kind = $4 AND check_status = 'container_checked'
             )",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .bind(kind)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error_message)?;
        Ok(present)
    }

    pub(crate) async fn span_is_archived(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<bool, DataError> {
        let present = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM risk_datasets
                WHERE exchange = $1 AND clearing_corporation = $2 AND trading_date = $3
                  AND origin = 'official' AND file_format = $4 AND parser_version = $5
                  AND publication_status IN ('draft', 'published')
                  AND validation_status <> 'failed'
             )",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .bind(SPAN_FILE_FORMAT)
        .bind(PARSER_VERSION)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error_message)?;
        Ok(present)
    }
}

fn choose_span_as_of(
    candidates: &[SpanCandidate],
    trading_datetime: NaiveDateTime,
) -> Result<(&SpanCandidate, NaiveDateTime), DataError> {
    let trading_date = trading_datetime.date();
    if candidates.is_empty() {
        return Err(DataError::MarginNotFound {
            detail: format!("no official SPAN file is archived for {trading_date}"),
        });
    }
    let mut eligible = Vec::new();
    for candidate in candidates {
        let created = span_created_timestamp(&candidate.prefix).map_err(|detail| {
            DataError::MarginRejected {
                detail: format!("{}: {detail}", candidate.original_filename),
            }
        })?;
        if created.date() != trading_date {
            return Err(DataError::MarginRejected {
                detail: format!(
                    "{} was created at {created}, which is not trading date {trading_date}",
                    candidate.original_filename
                ),
            });
        }
        if created <= trading_datetime {
            eligible.push((created, candidate));
        }
    }
    if eligible.is_empty() {
        return Err(DataError::MarginNotFound {
            detail: format!(
                "no SPAN file published at or before {trading_datetime} is archived for {trading_date}"
            ),
        });
    }
    let latest = eligible
        .iter()
        .map(|(created, _)| *created)
        .max()
        .expect("eligible");
    let top: Vec<&SpanCandidate> = eligible
        .into_iter()
        .filter(|(created, _)| *created == latest)
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
            detail: format!("the SPAN file created at {latest} has more than one file hash"),
        });
    }
    let chosen = top[0];
    if chosen.publication_status == "rejected" || chosen.validation_status == "failed" {
        return Err(DataError::MarginRejected {
            detail: format!(
                "the SPAN snapshot {} current at {latest} is {} and {}; an older snapshot was not substituted",
                chosen.original_filename, chosen.publication_status, chosen.validation_status
            ),
        });
    }
    if chosen.publication_status != "draft" && chosen.publication_status != "published" {
        return Err(DataError::MarginRejected {
            detail: format!(
                "the SPAN snapshot {} current at {latest} has publication status {}",
                chosen.original_filename, chosen.publication_status
            ),
        });
    }
    Ok((chosen, latest))
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
    use super::{DatasetRow, SpanCandidate, choose_span_as_of, dataset_from_row};
    use crate::DataError;
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
    fn the_latest_span_file_at_or_before_the_trade_time_is_chosen() {
        let early = span_file(
            "nsccl.20261028.i1.zip",
            "aa",
            "begin_of_day",
            None,
            "202610280201",
        );
        let mid = span_file(
            "nsccl.20261028.i4.zip",
            "bb",
            "intraday",
            Some(3),
            "202610281403",
        );
        let late = span_file(
            "nsccl.20261028.s.zip",
            "cc",
            "end_of_day",
            None,
            "202610281703",
        );
        let files = [early, mid, late];
        let afternoon = stamp("2026-10-28T14:30:00");
        let chosen = choose_span_as_of(&files, afternoon).expect("afternoon");
        assert_eq!(chosen.0.original_filename, "nsccl.20261028.i4.zip");
        assert_eq!(chosen.1, stamp("2026-10-28T14:03:00"));
        let evening = choose_span_as_of(&files, stamp("2026-10-28T18:00:00")).expect("evening");
        assert_eq!(evening.0.original_filename, "nsccl.20261028.s.zip");
        let before_open = choose_span_as_of(&files, stamp("2026-10-28T01:00:00"));
        assert!(matches!(before_open, Err(DataError::MarginNotFound { .. })));
        let mut rejected = files;
        rejected[1].publication_status = "rejected".to_owned();
        let blocked = choose_span_as_of(&rejected, afternoon);
        assert!(matches!(blocked, Err(DataError::MarginRejected { .. })));
    }

    fn stamp(value: &str) -> chrono::NaiveDateTime {
        chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").expect("stamp")
    }

    fn span_file(
        name: &str,
        hash: &str,
        class: &str,
        sequence: Option<i32>,
        created: &str,
    ) -> SpanCandidate {
        let payload = format!("<spanFile><created>{created}</created></spanFile>");
        let bytes = payload.as_bytes();
        let member = b"member.spn";
        let mut prefix = Vec::new();
        prefix.extend_from_slice(b"PK\x03\x04");
        prefix.extend_from_slice(&20_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u32.to_le_bytes());
        prefix.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        prefix.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        prefix.extend_from_slice(&(member.len() as u16).to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(member);
        prefix.extend_from_slice(bytes);
        SpanCandidate {
            id: Uuid::nil(),
            snapshot_class: class.to_owned(),
            intraday_sequence: sequence,
            original_filename: name.to_owned(),
            sha256: hash.to_owned(),
            publication_status: "draft".to_owned(),
            validation_status: "pending".to_owned(),
            prefix,
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
