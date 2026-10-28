use crate::ContractMaster;
use crate::archive::{
    AuxiliaryArchiveRequest, IngestReceipt, SPAN_SIGN, SpanArchiveRequest, archive_auxiliary,
    archive_span, record_download_failure,
};
use crate::download::file_not_published;
use crate::nse::fetch::{download_cash_var, download_contract_master, download_nse_report};
use crate::nse::reports::{
    NSE_SOURCE_ID, NseReportKind, SPAN_FILE_FORMAT, check_contract_elm_gzip, check_exposure_csv,
    check_span_container, is_verified_reports_endpoint,
};
use crate::span::quote::CashMarginRates;
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
    let client = build_download_client(&config.download)?;
    ingest_with_client(config, store, &client, kind, trading_date).await
}

pub async fn ensure_nse_margin_files(
    config: &AppConfig,
    store: &PgStore,
    trading_date: NaiveDate,
) -> Result<(), DataError> {
    let source = verified_nse_source(config)?;
    let client = build_download_client(&config.download)?;
    let mut required_error = None;
    for kind in NseReportKind::margin_files() {
        if store
            .report_is_archived(
                source.exchange(),
                source.clearing_corporation(),
                trading_date,
                kind,
            )
            .await?
        {
            continue;
        }
        match ingest_with_client(config, store, &client, kind, trading_date).await {
            Ok(receipt) if receipt.accepted => {}
            Ok(receipt) => {
                required_error.get_or_insert(DataError::ArchiveRejected {
                    detail: receipt.note,
                });
            }
            Err(error) if file_not_published(&error) && kind.is_span() => {
                tracing::info!(
                    report = kind.code(),
                    %trading_date,
                    "SPAN snapshot is not published for this date"
                );
            }
            Err(error) if file_not_published(&error) => {
                required_error.get_or_insert(error);
            }
            Err(error) => return Err(error),
        }
    }
    if !store
        .auxiliary_is_archived(
            source.exchange(),
            source.clearing_corporation(),
            trading_date,
            "cash_var_dat",
        )
        .await?
        && let Err(error) = archive_cash_var(config, store, &client, trading_date).await
        && !file_not_published(&error)
    {
        return Err(error);
    }
    if !store
        .auxiliary_is_archived(
            source.exchange(),
            source.clearing_corporation(),
            trading_date,
            "contract_master_gzip",
        )
        .await?
        && let Err(error) = archive_contract_master(config, store, &client, trading_date).await
    {
        return Err(error);
    }
    if !store
        .span_is_archived(
            source.exchange(),
            source.clearing_corporation(),
            trading_date,
        )
        .await?
    {
        return Err(DataError::MarginNotFound {
            detail: format!(
                "no official SPAN file is archived for {trading_date}, and NSE did not publish one"
            ),
        });
    }
    if let Some(error) = required_error {
        return Err(error);
    }
    Ok(())
}

async fn archive_cash_var(
    config: &AppConfig,
    store: &PgStore,
    client: &reqwest::Client,
    trading_date: NaiveDate,
) -> Result<(), DataError> {
    let source = verified_nse_source(config)?;
    let mut missing = None;
    for batch in (1..=6).rev() {
        match download_cash_var(client, &config.download, trading_date, batch).await {
            Ok(downloaded) => {
                let rejection = CashMarginRates::parse(&downloaded.bytes, trading_date).err();
                let receipt = archive_auxiliary(
                    store.pool(),
                    AuxiliaryArchiveRequest {
                        source_id: source.id().to_owned(),
                        exchange: source.exchange(),
                        clearing_corporation: source.clearing_corporation(),
                        trading_date,
                        kind_code: "cash-var".to_owned(),
                        auxiliary_kind: "cash_var_dat".to_owned(),
                        filename: downloaded.filename,
                        source_host: downloaded.source_host,
                        bytes: downloaded.bytes,
                        rejection,
                    },
                )
                .await?;
                if !receipt.accepted {
                    return Err(DataError::ArchiveRejected {
                        detail: receipt.note,
                    });
                }
                return Ok(());
            }
            Err(error) if file_not_published(&error) => missing = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(missing.unwrap_or(DataError::DownloadRejected {
        detail: format!("no official file was published for cash-var on {trading_date}"),
    }))
}

async fn archive_contract_master(
    config: &AppConfig,
    store: &PgStore,
    client: &reqwest::Client,
    trading_date: NaiveDate,
) -> Result<(), DataError> {
    let source = verified_nse_source(config)?;
    let downloaded = download_contract_master(client, &config.download, trading_date).await?;
    let rejection = ContractMaster::parse_gzip(&downloaded.bytes).err();
    let receipt = archive_auxiliary(
        store.pool(),
        AuxiliaryArchiveRequest {
            source_id: source.id().to_owned(),
            exchange: source.exchange(),
            clearing_corporation: source.clearing_corporation(),
            trading_date,
            kind_code: "contract-master".to_owned(),
            auxiliary_kind: "contract_master_gzip".to_owned(),
            filename: downloaded.filename,
            source_host: downloaded.source_host,
            bytes: downloaded.bytes,
            rejection,
        },
    )
    .await?;
    if receipt.accepted {
        Ok(())
    } else {
        Err(DataError::ArchiveRejected {
            detail: receipt.note,
        })
    }
}

async fn ingest_with_client(
    config: &AppConfig,
    store: &PgStore,
    client: &reqwest::Client,
    kind: NseReportKind,
    trading_date: NaiveDate,
) -> Result<IngestReceipt, DataError> {
    let source = verified_nse_source(config)?;
    let downloaded = match download_nse_report(client, &config.download, kind, trading_date).await {
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
                sign_convention: SPAN_SIGN.to_owned(),
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
