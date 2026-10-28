use crate::DataError;
use crate::hash::sha256_hex;
use crate::postgres::{DatasetRow, dataset_from_row, db_error_message, select_columns};
use crate::redact::sanitize_audit_fields;
use crate::span::xml::PARSER_VERSION;
use chrono::{DateTime, Utc};
use margin_domain::{
    ClearingCorporation, DatasetId, DatasetOrigin, Exchange, PublicationStatus, RiskDataset,
    RiskDatasetDraft, SnapshotLocator, ValidationStatus,
};
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) const SPAN_SIGN: &str =
    "NSE Clearing SPAN risk array: loss is positive and gain is negative";
const AUXILIARY_NOTE: &str = "original bytes archived; values were not applied as margin parameters; this file is not a margin";
const DUPLICATE_NOTE: &str = "existing bytes were reused; no second version was stored";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestReceipt {
    pub disposition: &'static str,
    pub record: &'static str,
    pub id: String,
    pub kind: String,
    pub filename: String,
    pub sha256: String,
    pub publication_status: Option<String>,
    pub validation_status: Option<String>,
    pub check_status: Option<String>,
    pub parser_status: String,
    pub note: String,
    pub accepted: bool,
}

pub(crate) struct SpanArchiveRequest {
    pub source_id: String,
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub trading_date: chrono::NaiveDate,
    pub snapshot: SnapshotLocator,
    pub filename: String,
    pub source_host: String,
    pub bytes: Vec<u8>,
    pub file_format: String,
    pub kind: String,
    pub rejection: Option<String>,
    pub scan_points: Vec<u32>,
    pub risk_array_count: u64,
    pub sign_convention: String,
}

pub(crate) struct AuxiliaryArchiveRequest {
    pub source_id: String,
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub trading_date: chrono::NaiveDate,
    pub kind_code: String,
    pub auxiliary_kind: String,
    pub filename: String,
    pub source_host: String,
    pub bytes: Vec<u8>,
    pub rejection: Option<String>,
}

pub(crate) async fn archive_span(
    pool: &PgPool,
    request: SpanArchiveRequest,
) -> Result<IngestReceipt, DataError> {
    let sha = sha256_hex(&request.bytes);
    if let Some(existing) = find_dataset(pool, sha.as_str()).await? {
        let receipt = span_receipt(
            "duplicate",
            &request.kind,
            &existing,
            DUPLICATE_NOTE.to_owned(),
        );
        record_run(
            pool,
            &RunWrite {
                source_id: &request.source_id,
                exchange: request.exchange,
                clearing_corporation: request.clearing_corporation,
                trading_date: request.trading_date,
                status: "succeeded",
                dataset_id: Some(existing.id()),
                auxiliary_file_id: None,
                error_summary: None,
                action: "ingest_duplicate",
                kind: &request.kind,
                filename: &request.filename,
                sha256: sha.as_str(),
            },
        )
        .await?;
        return Ok(receipt);
    }

    let rejected = request
        .rejection
        .as_ref()
        .map(|summary| bounded_summary(summary));
    let dataset = RiskDataset::try_new(RiskDatasetDraft {
        id: DatasetId::new(),
        exchange: request.exchange,
        clearing_corporation: request.clearing_corporation,
        trading_date: request.trading_date,
        snapshot: request.snapshot,
        publication_timestamp: None,
        download_timestamp: Utc::now(),
        original_filename: request.filename.clone(),
        source_host: Some(request.source_host.clone()),
        sha256: sha.clone(),
        file_format: request.file_format.clone(),
        parser_version: PARSER_VERSION.to_owned(),
        contract_master_version_id: None,
        validation_status: if rejected.is_some() {
            ValidationStatus::Failed
        } else {
            ValidationStatus::Pending
        },
        validation_error_summary: rejected.clone(),
        publication_status: if rejected.is_some() {
            PublicationStatus::Rejected
        } else {
            PublicationStatus::Draft
        },
        origin: DatasetOrigin::Official,
    })?;
    let note = rejected
        .clone()
        .unwrap_or_else(|| accepted_span_note(request.scan_points.len(), request.risk_array_count));
    let receipt = span_receipt("archived", &request.kind, &dataset, note);
    match insert_span(pool, &request, &dataset).await {
        Ok(()) => Ok(receipt),
        Err(InsertError::Unique) => {
            let Some(existing) = find_dataset(pool, sha.as_str()).await? else {
                return Err(DataError::ArchiveRejected {
                    detail: "these bytes were stored concurrently; retry the ingest".to_owned(),
                });
            };
            let receipt = span_receipt(
                "duplicate",
                &request.kind,
                &existing,
                DUPLICATE_NOTE.to_owned(),
            );
            record_run(
                pool,
                &RunWrite {
                    source_id: &request.source_id,
                    exchange: request.exchange,
                    clearing_corporation: request.clearing_corporation,
                    trading_date: request.trading_date,
                    status: "succeeded",
                    dataset_id: Some(existing.id()),
                    auxiliary_file_id: None,
                    error_summary: None,
                    action: "ingest_duplicate",
                    kind: &request.kind,
                    filename: &request.filename,
                    sha256: sha.as_str(),
                },
            )
            .await?;
            Ok(receipt)
        }
        Err(InsertError::Data(error)) => Err(error),
    }
}

pub(crate) async fn archive_auxiliary(
    pool: &PgPool,
    request: AuxiliaryArchiveRequest,
) -> Result<IngestReceipt, DataError> {
    let sha = sha256_hex(&request.bytes);
    if let Some(existing) = find_auxiliary(pool, sha.as_str()).await? {
        let receipt = auxiliary_receipt(
            "duplicate",
            &request,
            existing.id,
            &existing.check_status,
            DUPLICATE_NOTE,
        );
        record_run(
            pool,
            &RunWrite {
                source_id: &request.source_id,
                exchange: request.exchange,
                clearing_corporation: request.clearing_corporation,
                trading_date: request.trading_date,
                status: "succeeded",
                dataset_id: None,
                auxiliary_file_id: Some(existing.id),
                error_summary: None,
                action: "ingest_duplicate",
                kind: &request.kind_code,
                filename: &request.filename,
                sha256: sha.as_str(),
            },
        )
        .await?;
        return Ok(receipt);
    }
    let id = Uuid::now_v7();
    let rejected = request
        .rejection
        .as_ref()
        .map(|summary| bounded_summary(summary));
    let check_status = if rejected.is_some() {
        "rejected"
    } else {
        "container_checked"
    };
    let note = rejected
        .clone()
        .unwrap_or_else(|| AUXILIARY_NOTE.to_owned());
    let receipt = IngestReceipt {
        disposition: "archived",
        record: "auxiliary_file",
        id: id.to_string(),
        kind: request.kind_code.clone(),
        filename: request.filename.clone(),
        sha256: sha.as_str().to_owned(),
        publication_status: None,
        validation_status: None,
        check_status: Some(check_status.to_owned()),
        parser_status: "not_implemented".to_owned(),
        note,
        accepted: rejected.is_none(),
    };
    match insert_auxiliary(
        pool,
        &request,
        id,
        sha.as_str(),
        check_status,
        rejected.as_deref(),
    )
    .await
    {
        Ok(()) => Ok(receipt),
        Err(InsertError::Unique) => {
            let Some(existing) = find_auxiliary(pool, sha.as_str()).await? else {
                return Err(DataError::ArchiveRejected {
                    detail: "these bytes were stored concurrently; retry the ingest".to_owned(),
                });
            };
            let receipt = auxiliary_receipt(
                "duplicate",
                &request,
                existing.id,
                &existing.check_status,
                DUPLICATE_NOTE,
            );
            record_run(
                pool,
                &RunWrite {
                    source_id: &request.source_id,
                    exchange: request.exchange,
                    clearing_corporation: request.clearing_corporation,
                    trading_date: request.trading_date,
                    status: "succeeded",
                    dataset_id: None,
                    auxiliary_file_id: Some(existing.id),
                    error_summary: None,
                    action: "ingest_duplicate",
                    kind: &request.kind_code,
                    filename: &request.filename,
                    sha256: sha.as_str(),
                },
            )
            .await?;
            Ok(receipt)
        }
        Err(InsertError::Data(error)) => Err(error),
    }
}

pub(crate) async fn record_download_failure(
    pool: &PgPool,
    source_id: &str,
    exchange: Exchange,
    clearing_corporation: ClearingCorporation,
    trading_date: chrono::NaiveDate,
    kind: &str,
    error: &DataError,
) -> Result<(), DataError> {
    let summary = bounded_summary(&error.to_string());
    record_run(
        pool,
        &RunWrite {
            source_id,
            exchange,
            clearing_corporation,
            trading_date,
            status: "failed",
            dataset_id: None,
            auxiliary_file_id: None,
            error_summary: Some(&summary),
            action: "download_rejected",
            kind,
            filename: "",
            sha256: "",
        },
    )
    .await
}

enum InsertError {
    Unique,
    Data(DataError),
}

async fn insert_span(
    pool: &PgPool,
    request: &SpanArchiveRequest,
    dataset: &RiskDataset,
) -> Result<(), InsertError> {
    let mut tx = pool.begin().await.map_err(map_db)?;
    let file_id = Uuid::now_v7();
    let byte_size = i64::try_from(request.bytes.len()).map_err(|_| {
        InsertError::Data(DataError::ArchiveRejected {
            detail: "file is too large to store".to_owned(),
        })
    })?;
    let inserted = sqlx::query(
        "INSERT INTO risk_files (id, original_filename, sha256, byte_size, content) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(file_id)
    .bind(dataset.original_filename())
    .bind(dataset.sha256().as_str())
    .bind(byte_size)
    .bind(request.bytes.as_slice())
    .execute(&mut *tx)
    .await;
    if let Err(error) = inserted {
        return Err(map_insert(error));
    }
    let sequence = dataset
        .snapshot()
        .intraday_sequence()
        .map(i32::try_from)
        .transpose()
        .map_err(|_| {
            InsertError::Data(DataError::ArchiveRejected {
                detail: "intraday sequence does not fit".to_owned(),
            })
        })?;
    let inserted = sqlx::query(
        "INSERT INTO risk_datasets ( \
            id, exchange, clearing_corporation, trading_date, snapshot_class, \
            intraday_sequence, publication_timestamp, download_timestamp, original_filename, \
            source_host, sha256, file_format, parser_version, contract_master_version_id, \
            validation_status, validation_error_summary, publication_status, origin, risk_file_id \
         ) VALUES ( \
            $1, $2, $3, $4, $5, \
            $6, NULL, $7, $8, \
            $9, $10, $11, $12, NULL, \
            $13, $14, $15, $16, $17 \
         )",
    )
    .bind(dataset.id().as_uuid())
    .bind(dataset.exchange().code())
    .bind(dataset.clearing_corporation().code())
    .bind(dataset.trading_date())
    .bind(dataset.snapshot().class().code())
    .bind(sequence)
    .bind(dataset.download_timestamp())
    .bind(dataset.original_filename())
    .bind(dataset.source_host())
    .bind(dataset.sha256().as_str())
    .bind(dataset.file_format())
    .bind(dataset.parser_version())
    .bind(dataset.validation_status().code())
    .bind(dataset.validation_error_summary())
    .bind(dataset.publication_status().code())
    .bind(dataset.origin().code())
    .bind(file_id)
    .execute(&mut *tx)
    .await;
    if let Err(error) = inserted {
        return Err(map_insert(error));
    }
    if dataset.validation_status() != ValidationStatus::Failed {
        insert_scan_points(
            &mut tx,
            dataset.id(),
            &request.scan_points,
            &request.sign_convention,
        )
        .await?;
    }
    let status = if dataset.validation_status() == ValidationStatus::Failed {
        "rejected"
    } else {
        "succeeded"
    };
    insert_run_on(
        &mut tx,
        &RunWrite {
            source_id: &request.source_id,
            exchange: request.exchange,
            clearing_corporation: request.clearing_corporation,
            trading_date: request.trading_date,
            status,
            dataset_id: Some(dataset.id()),
            auxiliary_file_id: None,
            error_summary: dataset.validation_error_summary(),
            action: "archive_risk_file",
            kind: &request.kind,
            filename: dataset.original_filename(),
            sha256: dataset.sha256().as_str(),
        },
    )
    .await?;
    tx.commit().await.map_err(map_db)?;
    Ok(())
}

async fn insert_auxiliary(
    pool: &PgPool,
    request: &AuxiliaryArchiveRequest,
    id: Uuid,
    sha: &str,
    check_status: &str,
    check_summary: Option<&str>,
) -> Result<(), InsertError> {
    let mut tx = pool.begin().await.map_err(map_db)?;
    let byte_size = i64::try_from(request.bytes.len()).map_err(|_| {
        InsertError::Data(DataError::ArchiveRejected {
            detail: "file is too large to store".to_owned(),
        })
    })?;
    let downloaded_at = Utc::now();
    let inserted = sqlx::query(
        "INSERT INTO official_auxiliary_files ( \
            id, kind, exchange, clearing_corporation, trading_date, original_filename, \
            source_host, sha256, byte_size, content, download_timestamp, check_status, \
            check_summary, parser_status \
         ) VALUES ( \
            $1, $2, $3, $4, $5, $6, \
            $7, $8, $9, $10, $11, $12, \
            $13, 'not_implemented' \
         )",
    )
    .bind(id)
    .bind(&request.auxiliary_kind)
    .bind(request.exchange.code())
    .bind(request.clearing_corporation.code())
    .bind(request.trading_date)
    .bind(&request.filename)
    .bind(&request.source_host)
    .bind(sha)
    .bind(byte_size)
    .bind(request.bytes.as_slice())
    .bind(downloaded_at)
    .bind(check_status)
    .bind(check_summary)
    .execute(&mut *tx)
    .await;
    if let Err(error) = inserted {
        return Err(map_insert(error));
    }
    let status = if check_status == "rejected" {
        "rejected"
    } else {
        "succeeded"
    };
    insert_run_on(
        &mut tx,
        &RunWrite {
            source_id: &request.source_id,
            exchange: request.exchange,
            clearing_corporation: request.clearing_corporation,
            trading_date: request.trading_date,
            status,
            dataset_id: None,
            auxiliary_file_id: Some(id),
            error_summary: check_summary,
            action: "archive_auxiliary_file",
            kind: &request.kind_code,
            filename: &request.filename,
            sha256: sha,
        },
    )
    .await?;
    tx.commit().await.map_err(map_db)?;
    Ok(())
}

struct RunWrite<'a> {
    source_id: &'a str,
    exchange: Exchange,
    clearing_corporation: ClearingCorporation,
    trading_date: chrono::NaiveDate,
    status: &'a str,
    dataset_id: Option<DatasetId>,
    auxiliary_file_id: Option<Uuid>,
    error_summary: Option<&'a str>,
    action: &'a str,
    kind: &'a str,
    filename: &'a str,
    sha256: &'a str,
}

async fn record_run(pool: &PgPool, run: &RunWrite<'_>) -> Result<(), DataError> {
    let mut tx = pool.begin().await.map_err(db_error_message)?;
    insert_run_on(&mut tx, run)
        .await
        .map_err(|error| match error {
            InsertError::Data(error) => error,
            InsertError::Unique => DataError::ArchiveRejected {
                detail: "ingestion run conflicted with an existing row".to_owned(),
            },
        })?;
    tx.commit().await.map_err(db_error_message)?;
    Ok(())
}

async fn insert_run_on(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run: &RunWrite<'_>,
) -> Result<(), InsertError> {
    let run_id = Uuid::now_v7();
    let now: DateTime<Utc> = Utc::now();
    sqlx::query(
        "INSERT INTO risk_ingestion_runs ( \
            id, source_id, exchange, clearing_corporation, trading_date, \
            started_at, finished_at, status, dataset_id, auxiliary_file_id, error_summary \
         ) VALUES ($1, $2, $3, $4, $5, $6, $6, $7, $8, $9, $10)",
    )
    .bind(run_id)
    .bind(run.source_id)
    .bind(run.exchange.code())
    .bind(run.clearing_corporation.code())
    .bind(run.trading_date)
    .bind(now)
    .bind(run.status)
    .bind(run.dataset_id.map(DatasetId::as_uuid))
    .bind(run.auxiliary_file_id)
    .bind(run.error_summary)
    .execute(&mut **tx)
    .await
    .map_err(map_insert)?;
    let entity_id = run
        .dataset_id
        .map(|id| id.to_string())
        .or_else(|| run.auxiliary_file_id.map(|id| id.to_string()))
        .unwrap_or_else(|| run_id.to_string());
    let entity_type = if run.dataset_id.is_some() {
        "risk_dataset"
    } else if run.auxiliary_file_id.is_some() {
        "official_auxiliary_file"
    } else {
        "risk_ingestion_run"
    };
    sqlx::query(
        "INSERT INTO audit_events (id, actor, action, entity_type, entity_id, detail) \
         VALUES ($1, 'margin-engine', $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(run.action)
    .bind(entity_type)
    .bind(entity_id)
    .bind(sqlx::types::Json(audit_detail(&[
        ("kind", run.kind),
        ("filename", run.filename),
        ("sha256", run.sha256),
        ("status", run.status),
    ])))
    .execute(&mut **tx)
    .await
    .map_err(map_insert)?;
    Ok(())
}

fn audit_detail(pairs: &[(&str, &str)]) -> serde_json::Value {
    let mut fields = BTreeMap::new();
    for (key, value) in pairs {
        fields.insert((*key).to_owned(), (*value).to_owned());
    }
    let clean = sanitize_audit_fields(fields);
    serde_json::Value::Object(
        clean
            .into_iter()
            .map(|(key, value)| (key, serde_json::Value::String(value)))
            .collect(),
    )
}

async fn find_dataset(pool: &PgPool, sha256: &str) -> Result<Option<RiskDataset>, DataError> {
    let row = sqlx::query_as::<_, DatasetRow>(&format!("{} WHERE sha256 = $1", select_columns()))
        .bind(sha256)
        .fetch_optional(pool)
        .await
        .map_err(db_error_message)?;
    row.map(dataset_from_row).transpose()
}

#[derive(sqlx::FromRow)]
struct AuxiliaryRow {
    id: Uuid,
    check_status: String,
}

async fn find_auxiliary(pool: &PgPool, sha256: &str) -> Result<Option<AuxiliaryRow>, DataError> {
    sqlx::query_as::<_, AuxiliaryRow>(
        "SELECT id, check_status FROM official_auxiliary_files WHERE sha256 = $1",
    )
    .bind(sha256)
    .fetch_optional(pool)
    .await
    .map_err(db_error_message)
}

fn accepted_span_note(scan_points: usize, risk_arrays: u64) -> String {
    if scan_points == 0 {
        format!(
            "XML-based SPAN 4.00 parsed; {risk_arrays} risk arrays checked across more than one rate; scan-point rows were not stored; values stay in the archived file; this dataset is not a margin"
        )
    } else {
        format!(
            "XML-based SPAN 4.00 parsed; {scan_points} scan points stored; {risk_arrays} risk arrays checked and left in the archived file because it has no lot size or index/stock class; this dataset is not a margin"
        )
    }
}

async fn insert_scan_points(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dataset_id: margin_domain::DatasetId,
    points: &[u32],
    sign_convention: &str,
) -> Result<(), InsertError> {
    for point in points {
        let index = i32::try_from(*point).map_err(|_| {
            InsertError::Data(DataError::ArchiveRejected {
                detail: "scan point does not fit".to_owned(),
            })
        })?;
        sqlx::query(
            "INSERT INTO scenario_definitions \
             (id, dataset_id, scenario_index, scenario_code, pnl_convention, sign_convention) \
             VALUES ($1, $2, $3, $4, 'loss', $5)",
        )
        .bind(Uuid::now_v7())
        .bind(dataset_id.as_uuid())
        .bind(index)
        .bind(format!("scan-point-{point}"))
        .bind(sign_convention)
        .execute(&mut **tx)
        .await
        .map_err(map_insert)?;
    }
    Ok(())
}

fn span_receipt(
    disposition: &'static str,
    kind: &str,
    dataset: &RiskDataset,
    note: String,
) -> IngestReceipt {
    let publication = dataset.publication_status().code();
    IngestReceipt {
        disposition,
        record: "dataset",
        id: dataset.id().to_string(),
        kind: kind.to_owned(),
        filename: dataset.original_filename().to_owned(),
        sha256: dataset.sha256().as_str().to_owned(),
        publication_status: Some(publication.to_owned()),
        validation_status: Some(dataset.validation_status().code().to_owned()),
        check_status: None,
        parser_status: dataset.parser_version().to_owned(),
        note,
        accepted: publication == "draft" || publication == "published",
    }
}

fn auxiliary_receipt(
    disposition: &'static str,
    request: &AuxiliaryArchiveRequest,
    id: Uuid,
    check_status: &str,
    note: &str,
) -> IngestReceipt {
    IngestReceipt {
        disposition,
        record: "auxiliary_file",
        id: id.to_string(),
        kind: request.kind_code.clone(),
        filename: request.filename.clone(),
        sha256: sha256_hex(&request.bytes).as_str().to_owned(),
        publication_status: None,
        validation_status: None,
        check_status: Some(check_status.to_owned()),
        parser_status: "not_implemented".to_owned(),
        note: note.to_owned(),
        accepted: check_status == "container_checked",
    }
}

fn bounded_summary(summary: &str) -> String {
    let trimmed = summary.trim();
    let text = if trimmed.is_empty() {
        "container check failed"
    } else {
        trimmed
    };
    text.chars().take(500).collect()
}

fn map_db(error: sqlx::Error) -> InsertError {
    InsertError::Data(db_error_message(error))
}

fn map_insert(error: sqlx::Error) -> InsertError {
    if error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
    {
        InsertError::Unique
    } else {
        map_db(error)
    }
}

#[cfg(test)]
mod tests {
    use super::{AuxiliaryArchiveRequest, SpanArchiveRequest, archive_auxiliary, archive_span};
    use crate::nse::reports::SPAN_FILE_FORMAT;
    use crate::span::xml::check_span_container;
    use chrono::NaiveDate;
    use margin_domain::{ClearingCorporation, Exchange, SnapshotLocator};
    use std::io::{Cursor, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    #[tokio::test]
    async fn archived_span_stays_unpublished_and_bytes_stay_immutable() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            eprintln!("DATABASE_URL is unset; skipping archive integration test");
            return;
        };
        if url.trim().is_empty() {
            eprintln!("DATABASE_URL is empty; skipping archive integration test");
            return;
        }
        let pool = crate::connect(&url, 2, std::time::Duration::from_secs(5))
            .await
            .expect("connect");
        crate::migrate(&pool).await.expect("migrate");
        let before = official_published(&pool).await;

        let member = "nsccl.20261027.s.spn";
        let trading_date = NaiveDate::from_ymd_opt(2026, 10, 27).expect("date");
        let bytes = span_zip(
            member,
            br#"<?xml version="1.0"?><spanFile><fileFormat>4.00</fileFormat><pointInTime><date>20261027</date></pointInTime><clearingOrg><ec>NSCCL</ec></clearingOrg><pointDef><r>1</r><scanPointDef><point>1</point></scanPointDef><scanPointDef><point>2</point></scanPointDef></pointDef><exchange><exch>NSE</exch><fut><ra><r>1</r><a>1.50</a><a>-0.25</a></ra></fut></exchange></spanFile>"#,
        );
        let report = check_span_container(&bytes, member, 8192, trading_date).expect("span");
        assert_eq!(report.scan_points, vec![1, 2]);
        let request = SpanArchiveRequest {
            source_id: "nse-clearing-equity-derivatives".to_owned(),
            exchange: Exchange::Nse,
            clearing_corporation: ClearingCorporation::Nscc,
            trading_date,
            snapshot: SnapshotLocator::end_of_day(),
            filename: "nsccl.20261027.s.zip".to_owned(),
            source_host: "www.nseindia.com".to_owned(),
            bytes: bytes.clone(),
            file_format: SPAN_FILE_FORMAT.to_owned(),
            kind: "span-end-of-day".to_owned(),
            rejection: None,
            scan_points: report.scan_points.clone(),
            risk_array_count: report.risk_array_count,
            sign_convention: super::SPAN_SIGN.to_owned(),
        };
        let first = archive_span(&pool, request).await.expect("archive");
        let second = archive_span(
            &pool,
            SpanArchiveRequest {
                source_id: "nse-clearing-equity-derivatives".to_owned(),
                exchange: Exchange::Nse,
                clearing_corporation: ClearingCorporation::Nscc,
                trading_date,
                snapshot: SnapshotLocator::end_of_day(),
                filename: "nsccl.20261027.s.zip".to_owned(),
                source_host: "www.nseindia.com".to_owned(),
                bytes: bytes.clone(),
                file_format: SPAN_FILE_FORMAT.to_owned(),
                kind: "span-end-of-day".to_owned(),
                rejection: None,
                scan_points: report.scan_points.clone(),
                risk_array_count: report.risk_array_count,
                sign_convention: super::SPAN_SIGN.to_owned(),
            },
        )
        .await
        .expect("duplicate");
        assert_eq!(first.id, second.id);
        assert_eq!(second.disposition, "duplicate");
        assert!(first.accepted);
        assert_eq!(first.publication_status.as_deref(), Some("draft"));
        assert_eq!(first.validation_status.as_deref(), Some("pending"));
        assert_eq!(first.parser_status, "span-xml-4.00");
        let stored_points: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM scenario_definitions WHERE dataset_id = $1")
                .bind(uuid::Uuid::parse_str(&first.id).expect("dataset id"))
                .fetch_one(&pool)
                .await
                .expect("scan points");
        assert_eq!(stored_points, 2);
        assert_eq!(official_published(&pool).await, before);

        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT content FROM risk_files WHERE sha256 = $1")
                .bind(&first.sha256)
                .fetch_one(&pool)
                .await
                .expect("bytes");
        assert_eq!(stored, bytes);
        let rewritten = sqlx::query("UPDATE risk_files SET content = content WHERE sha256 = $1")
            .bind(&first.sha256)
            .execute(&pool)
            .await;
        assert!(rewritten.is_err());

        let auxiliary = AuxiliaryArchiveRequest {
            source_id: "nse-clearing-equity-derivatives".to_owned(),
            exchange: Exchange::Nse,
            clearing_corporation: ClearingCorporation::Nscc,
            trading_date: NaiveDate::from_ymd_opt(2026, 10, 27).expect("date"),
            kind_code: "exposure-limit".to_owned(),
            auxiliary_kind: "exposure_limit_csv".to_owned(),
            filename: "ael_27102026.csv".to_owned(),
            source_host: "www.nseindia.com".to_owned(),
            bytes: b"synthetic-exposure-not-official\n".to_vec(),
            rejection: None,
        };
        let saved = archive_auxiliary(&pool, auxiliary)
            .await
            .expect("auxiliary");
        assert!(saved.accepted);
        assert_eq!(saved.check_status.as_deref(), Some("container_checked"));
        assert_eq!(saved.parser_status, "not_implemented");
        let again = archive_auxiliary(
            &pool,
            AuxiliaryArchiveRequest {
                source_id: "nse-clearing-equity-derivatives".to_owned(),
                exchange: Exchange::Nse,
                clearing_corporation: ClearingCorporation::Nscc,
                trading_date: NaiveDate::from_ymd_opt(2026, 10, 27).expect("date"),
                kind_code: "exposure-limit".to_owned(),
                auxiliary_kind: "exposure_limit_csv".to_owned(),
                filename: "ael_27102026.csv".to_owned(),
                source_host: "www.nseindia.com".to_owned(),
                bytes: b"synthetic-exposure-not-official\n".to_vec(),
                rejection: None,
            },
        )
        .await
        .expect("auxiliary duplicate");
        assert_eq!(again.disposition, "duplicate");
        assert_eq!(again.id, saved.id);
        let mutated =
            sqlx::query("UPDATE official_auxiliary_files SET content = content WHERE id = $1")
                .bind(uuid::Uuid::parse_str(&saved.id).expect("uuid"))
                .execute(&pool)
                .await;
        assert!(mutated.is_err());
        assert_eq!(official_published(&pool).await, before);
    }

    async fn official_published(pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM risk_datasets \
             WHERE publication_status = 'published' AND validation_status = 'passed' \
             AND origin = 'official'",
        )
        .fetch_one(pool)
        .await
        .expect("count")
    }

    fn span_zip(name: &str, contents: &[u8]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            writer
                .start_file(name, SimpleFileOptions::default())
                .expect("start");
            writer.write_all(contents).expect("write");
            writer.finish().expect("finish");
        }
        cursor.into_inner()
    }
}
