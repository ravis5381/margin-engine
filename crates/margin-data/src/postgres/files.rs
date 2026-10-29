//! Archived SPAN and auxiliary bytes for one trade date.

use super::{PgStore, db_error_message};
use crate::DataError;
use crate::nse::reports::{NseReportKind, SPAN_FILE_FORMAT};
use crate::span::xml::PARSER_VERSION;
use chrono::NaiveDate;
use margin_domain::{ClearingCorporation, Exchange};

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
        let rows = self
            .auxiliary_rows(
                exchange,
                clearing,
                trading_date,
                "contract_master_gzip",
                "container_checked",
            )
            .await?;
        if !rows.is_empty() {
            return one_auxiliary_hash(
                rows,
                format!("more than one contract master hash is archived for {trading_date}"),
            );
        }
        let Some(previous) = self
            .latest_auxiliary_date_before(exchange, clearing, trading_date, "contract_master_gzip")
            .await?
        else {
            return Err(DataError::MarginRejected {
                detail: format!(
                    "no contract master is archived for {trading_date}, and no earlier contract master is archived"
                ),
            });
        };
        tracing::info!(
            %trading_date,
            %previous,
            "contract master for this date is not archived; using the previous file"
        );
        self.one_auxiliary(
            exchange,
            clearing,
            previous,
            "contract_master_gzip",
            format!("no contract master is archived for {previous}"),
            format!("more than one contract master hash is archived for {previous}"),
        )
        .await
    }

    async fn latest_auxiliary_date_before(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
        kind: &str,
    ) -> Result<Option<NaiveDate>, DataError> {
        sqlx::query_scalar(
            "SELECT MAX(trading_date) FROM official_auxiliary_files \
             WHERE exchange = $1 AND clearing_corporation = $2 AND kind = $3 \
               AND check_status = 'container_checked' AND trading_date < $4",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(kind)
        .bind(trading_date)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error_message)
    }

    pub(crate) async fn cash_var_file(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_date: NaiveDate,
    ) -> Result<Option<Vec<u8>>, DataError> {
        let checked = self
            .auxiliary_rows(
                exchange,
                clearing,
                trading_date,
                "cash_var_dat",
                "container_checked",
            )
            .await?;
        let rows = if checked.is_empty() {
            self.auxiliary_rows(exchange, clearing, trading_date, "cash_var_dat", "rejected")
                .await?
        } else {
            checked
        };
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
