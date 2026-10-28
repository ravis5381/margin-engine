use crate::archive::{
    AuxiliaryArchiveRequest, IngestReceipt, SpanArchiveRequest, archive_auxiliary, archive_span,
    record_download_failure,
};
use crate::fetch::download_nse_report;
use crate::nse_reports::{
    NSE_SOURCE_ID, NseReportKind, SPAN_FILE_FORMAT, check_contract_elm_gzip, check_exposure_csv,
    check_span_container, is_verified_reports_endpoint,
};
use crate::{
    AppConfig, ConfiguredSource, DataError, PgStore, build_download_client, ensure_source_fetchable,
};
use chrono::NaiveDate;
use margin_domain::{ClearingCorporation, Exchange};

pub fn verified_nse_source(config: &AppConfig) -> Result<&ConfiguredSource, DataError> {
    let source = config
        .sources
        .iter()
        .find(|source| source.id() == NSE_SOURCE_ID)
        .ok_or_else(|| DataError::Configuration {
            detail: format!("source `{NSE_SOURCE_ID}` is not configured"),
        })?;
    ensure_source_fetchable(source, &config.download)?;
    let url = source.url().ok_or_else(|| DataError::Configuration {
        detail: "NSE source has no URL".to_owned(),
    })?;
    if !is_verified_reports_endpoint(url) {
        return Err(DataError::Configuration {
            detail: "NSE source URL must be https://www.nseindia.com/api/reports".to_owned(),
        });
    }
    if source.exchange() != Exchange::Nse
        || source.clearing_corporation() != ClearingCorporation::Nscc
    {
        return Err(DataError::Configuration {
            detail: "NSE report downloads require exchange NSE and clearing corporation NSCCL"
                .to_owned(),
        });
    }
    Ok(source)
}

pub async fn ingest_nse_report(
    config: &AppConfig,
    store: &PgStore,
    kind: NseReportKind,
    trading_date: NaiveDate,
) -> Result<IngestReceipt, DataError> {
    let source = verified_nse_source(config)?;
    let client = build_download_client(&config.download)?;
    let downloaded = match download_nse_report(&client, &config.download, kind, trading_date).await
    {
        Ok(downloaded) => downloaded,
        Err(error) => {
            if let Err(record_error) = record_download_failure(
                store.pool(),
                source.id(),
                source.exchange(),
                source.clearing_corporation(),
                trading_date,
                kind.code(),
                &error,
            )
            .await
            {
                tracing::error!(%record_error, "ingestion run was not recorded");
            }
            return Err(error);
        }
    };

    if let Some(member) = kind.expected_span_member(trading_date)? {
        let snapshot = kind.snapshot().ok_or_else(|| DataError::DownloadRejected {
            detail: "SPAN report is missing a snapshot class".to_owned(),
        })?;
        let parsed = check_span_container(
            &downloaded.bytes,
            &member,
            config.max_file_bytes,
            trading_date,
        );
        let (rejection, scan_points, risk_array_count) = match parsed {
            Ok(report) => (None, report.scan_points, report.risk_array_count),
            Err(error) => (Some(error), Vec::new(), 0),
        };
        return archive_span(
            store.pool(),
            SpanArchiveRequest {
                source_id: source.id().to_owned(),
                exchange: source.exchange(),
                clearing_corporation: source.clearing_corporation(),
                trading_date,
                snapshot,
                filename: downloaded.filename,
                source_host: downloaded.source_host,
                bytes: downloaded.bytes,
                file_format: SPAN_FILE_FORMAT.to_owned(),
                kind: kind.code().to_owned(),
                rejection,
                scan_points,
                risk_array_count,
            },
        )
        .await;
    }

    let auxiliary_kind = kind
        .auxiliary_kind()
        .ok_or_else(|| DataError::DownloadRejected {
            detail: format!("report `{}` has no archive kind", kind.code()),
        })?;
    let rejection = match kind {
        NseReportKind::ExposureLimit => check_exposure_csv(&downloaded.bytes, trading_date).err(),
        NseReportKind::ContractElm => {
            check_contract_elm_gzip(&downloaded.bytes, config.max_file_bytes).err()
        }
        _ => {
            return Err(DataError::DownloadRejected {
                detail: format!("report `{}` cannot be archived", kind.code()),
            });
        }
    };
    archive_auxiliary(
        store.pool(),
        AuxiliaryArchiveRequest {
            source_id: source.id().to_owned(),
            exchange: source.exchange(),
            clearing_corporation: source.clearing_corporation(),
            trading_date,
            kind_code: kind.code().to_owned(),
            auxiliary_kind: auxiliary_kind.to_owned(),
            filename: downloaded.filename,
            source_host: downloaded.source_host,
            bytes: downloaded.bytes,
            rejection,
        },
    )
    .await
}
