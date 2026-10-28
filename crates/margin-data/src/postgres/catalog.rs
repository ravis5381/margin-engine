//! Published dataset rows. The quote does not read this list.

use super::{PgStore, db_error_message};
use crate::DataError;
use crate::store::{DatasetPage, DatasetReadModel, summarize};
use chrono::{DateTime, NaiveDate, Utc};
use margin_domain::{
    ClearingCorporation, ContractMasterVersionId, DatasetId, DatasetOrigin, Exchange,
    PublicationStatus, RiskDataset, RiskDatasetDraft, Sha256Hex, SnapshotClass, SnapshotLocator,
    ValidationStatus,
};
use uuid::Uuid;

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
