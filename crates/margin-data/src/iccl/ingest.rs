//! ICCL equity-derivatives SPAN and extreme-loss files published by BSE.
//!
//! The risk-parameter page loads
//! `https://api.bseindia.com/BseIndiaAPI/api/LoadData/w?date=YYYYMMDD&flag=0`.
//! Each `File_Path` uses `http://notices.bseindia.com/`, and the page downloads
//! the same path from `https://www.bseindia.com/bsedata/`. The member is
//! XML-based SPAN 4.00. `flag=0` is the XML zip. The SPN zip is not parsed.
//! The extreme-loss file is `ICCL_EDXELMddmmyy.zip` on `www.bseindia.com`.
//! Both URLs were observed for 28 October 2026. The listing API denies a
//! non-browser client, so that denial uses the six public paths the listing
//! names (`-00`, `-01`, `-02`, `-03`, `-04`, and `-FINAL`). A missing path is
//! skipped.

use crate::archive::{
    AuxiliaryArchiveRequest, IngestReceipt, SpanArchiveRequest, archive_auxiliary, archive_span,
    record_download_failure,
};
use crate::download::{file_not_published, media_type_matches, read_response_body};
use crate::span::quote::ContractElmRates;
use crate::span::xml::check_span_container;
use crate::{
    AppConfig, ConfiguredSource, DataError, DownloadPolicy, PgStore, build_download_client,
    ensure_source_fetchable,
};
use chrono::{Datelike, NaiveDate};
use margin_domain::{ClearingCorporation, Exchange, SnapshotLocator};
use reqwest::Url;
use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;

pub const ICCL_SOURCE_ID: &str = "iccl-equity-derivatives-risk-parameters";
pub const ICCL_SPAN_FILE_FORMAT: &str = "iccl-span-xml-4.00-zip";
pub const ICCL_ELM_KIND: &str = "iccl_elm_zip";

const CATALOG_URL: &str = "https://api.bseindia.com/BseIndiaAPI/api/LoadData/w";
const NOTICES_PREFIX: &str = "http://notices.bseindia.com/";
const PUBLIC_PREFIX: &str = "https://www.bseindia.com/bsedata/";
const ELM_HOST: &str = "www.bseindia.com";
const USER_AGENT_VALUE: &str = "Mozilla/5.0";
const SPAN_SIGN: &str = "ICCL SPAN risk array: loss is positive and gain is negative";

pub fn verified_iccl_source(config: &AppConfig) -> Result<&ConfiguredSource, DataError> {
    let source = config
        .sources
        .iter()
        .find(|source| source.id() == ICCL_SOURCE_ID)
        .ok_or_else(|| DataError::Configuration {
            detail: format!("source `{ICCL_SOURCE_ID}` is not configured"),
        })?;
    ensure_source_fetchable(source, &config.download)?;
    let url = source.url().ok_or_else(|| DataError::Configuration {
        detail: "ICCL source has no URL".to_owned(),
    })?;
    if url.as_str() != CATALOG_URL {
        return Err(DataError::Configuration {
            detail: format!("ICCL source URL must be {CATALOG_URL}"),
        });
    }
    if source.exchange() != Exchange::Bse
        || source.clearing_corporation() != ClearingCorporation::Iccl
    {
        return Err(DataError::Configuration {
            detail: "ICCL downloads require exchange BSE and clearing corporation ICCL".to_owned(),
        });
    }
    Ok(source)
}

pub async fn ensure_iccl_margin_files(
    config: &AppConfig,
    store: &PgStore,
    trading_date: NaiveDate,
) -> Result<(), DataError> {
    let receipts = ingest_iccl_files(config, store, trading_date).await?;
    if let Some(receipt) = receipts.into_iter().find(|receipt| !receipt.accepted) {
        return Err(DataError::ArchiveRejected {
            detail: receipt.note,
        });
    }
    Ok(())
}

pub async fn ingest_iccl_files(
    config: &AppConfig,
    store: &PgStore,
    trading_date: NaiveDate,
) -> Result<Vec<IngestReceipt>, DataError> {
    let source = verified_iccl_source(config)?;
    let client = build_download_client(&config.download)?;
    let mut receipts = Vec::new();
    let mut archived_span = false;
    let catalog = match download_catalog(&client, &config.download, trading_date).await {
        Ok(rows) => rows,
        Err(error) => {
            let _ = record_download_failure(
                store.pool(),
                source.id(),
                Exchange::Bse,
                ClearingCorporation::Iccl,
                trading_date,
                "iccl-span-xml",
                &error,
            )
            .await;
            return Err(error);
        }
    };
    if catalog.is_empty() {
        return Err(DataError::DownloadRejected {
            detail: format!("no ICCL SPAN file is published for {trading_date}"),
        });
    }
    for row in catalog {
        let snapshot = row.snapshot;
        if store
            .span_snapshot_archived(
                Exchange::Bse,
                ClearingCorporation::Iccl,
                trading_date,
                ICCL_SPAN_FILE_FORMAT,
                &snapshot,
            )
            .await?
        {
            archived_span = true;
            continue;
        }
        let downloaded =
            match download_bytes(&client, &config.download, &row.url, &row.filename).await {
                Ok(bytes) => bytes,
                Err(error) if !row.required && file_not_published(&error) => continue,
                Err(error) => {
                    let _ = record_download_failure(
                        store.pool(),
                        source.id(),
                        Exchange::Bse,
                        ClearingCorporation::Iccl,
                        trading_date,
                        "iccl-span-xml",
                        &error,
                    )
                    .await;
                    return Err(error);
                }
            };
        let parsed = check_span_container(
            &downloaded,
            &row.member,
            config.max_file_bytes,
            trading_date,
        );
        let (rejection, scan_points, risk_array_count) = match parsed {
            Ok(report) => (None, report.scan_points, report.risk_array_count),
            Err(error) => (Some(error), Vec::new(), 0),
        };
        receipts.push(
            archive_span(
                store.pool(),
                SpanArchiveRequest {
                    source_id: source.id().to_owned(),
                    exchange: Exchange::Bse,
                    clearing_corporation: ClearingCorporation::Iccl,
                    trading_date,
                    snapshot,
                    filename: row.filename,
                    source_host: ELM_HOST.to_owned(),
                    bytes: downloaded,
                    file_format: ICCL_SPAN_FILE_FORMAT.to_owned(),
                    kind: "iccl-span-xml".to_owned(),
                    rejection,
                    scan_points,
                    risk_array_count,
                    sign_convention: SPAN_SIGN.to_owned(),
                },
            )
            .await?,
        );
        archived_span = true;
    }
    if !archived_span {
        return Err(DataError::DownloadRejected {
            detail: format!("no ICCL SPAN file is published for {trading_date}"),
        });
    }
    if !store
        .auxiliary_is_archived(
            Exchange::Bse,
            ClearingCorporation::Iccl,
            trading_date,
            ICCL_ELM_KIND,
        )
        .await?
    {
        receipts.push(archive_elm(config, store, source, &client, trading_date).await?);
    }
    Ok(receipts)
}

async fn archive_elm(
    config: &AppConfig,
    store: &PgStore,
    source: &ConfiguredSource,
    client: &reqwest::Client,
    trading_date: NaiveDate,
) -> Result<IngestReceipt, DataError> {
    let filename = format!("ICCL_EDXELM{}.zip", day_month_year(trading_date));
    let url = Url::parse(&format!("https://{ELM_HOST}/downloads/{filename}")).map_err(|_| {
        DataError::DownloadRejected {
            detail: "ICCL extreme loss URL is not valid".to_owned(),
        }
    })?;
    let bytes = match download_bytes(client, &config.download, &url, &filename).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = record_download_failure(
                store.pool(),
                source.id(),
                Exchange::Bse,
                ClearingCorporation::Iccl,
                trading_date,
                "iccl-elm",
                &error,
            )
            .await;
            return Err(error);
        }
    };
    let rejection = ContractElmRates::parse_iccl_zip(&bytes).err();
    let parsed = rejection.is_none();
    let receipt = archive_auxiliary(
        store.pool(),
        AuxiliaryArchiveRequest {
            source_id: source.id().to_owned(),
            exchange: Exchange::Bse,
            clearing_corporation: ClearingCorporation::Iccl,
            trading_date,
            kind_code: "iccl-elm".to_owned(),
            auxiliary_kind: ICCL_ELM_KIND.to_owned(),
            filename,
            source_host: ELM_HOST.to_owned(),
            bytes,
            rejection,
        },
    )
    .await?;
    if parsed && receipt.disposition == "duplicate" {
        return Ok(IngestReceipt {
            accepted: true,
            ..receipt
        });
    }
    Ok(receipt)
}

async fn download_catalog(
    client: &reqwest::Client,
    policy: &DownloadPolicy,
    trading_date: NaiveDate,
) -> Result<Vec<SpanPublication>, DataError> {
    let url = Url::parse(&format!(
        "{CATALOG_URL}?date={}&flag=0",
        trading_date.format("%Y%m%d")
    ))
    .map_err(|_| DataError::DownloadRejected {
        detail: "ICCL catalog URL is not valid".to_owned(),
    })?;
    policy.authorize(&url)?;
    let response = client
        .get(url)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .await
        .map_err(|error| DataError::DownloadRejected {
            detail: format!("ICCL catalog request failed: {}", error_detail(&error)),
        })?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        // Akamai denies this listing to a non-browser client. The page's listing
        // names these six public paths, and an unpublished snapshot is a 404.
        return public_span_publications(trading_date);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(DataError::DownloadRejected {
            detail: format!("ICCL catalog request returned HTTP {}", response.status()),
        });
    }
    if response.url().as_str().split('?').next() != Some(CATALOG_URL) {
        return Err(DataError::DownloadRejected {
            detail: "ICCL catalog response left the verified API path".to_owned(),
        });
    }
    let body = read_response_body(response, policy.max_bytes(), "ICCL").await?;
    let catalog: Catalog =
        serde_json::from_slice(&body).map_err(|_| DataError::DownloadRejected {
            detail: "ICCL catalog response is not the verified JSON listing".to_owned(),
        })?;
    let mut publications = Vec::new();
    for row in catalog.table {
        if row.file_type != "XML_FILE" {
            continue;
        }
        publications.push(publication(trading_date, &row)?);
    }
    Ok(publications)
}

fn publication(trading_date: NaiveDate, row: &CatalogRow) -> Result<SpanPublication, DataError> {
    let (suffix, snapshot) =
        snapshot_for_mode(&row.file_mode).ok_or_else(|| DataError::DownloadRejected {
            detail: format!(
                "ICCL file mode `{}` is not a published SPAN snapshot",
                row.file_mode
            ),
        })?;
    let filename = format!("BSERISK{}-{suffix}.ZIP", trading_date.format("%Y%m%d"));
    let url = public_span_url(&row.file_path, &filename)?;
    let member = format!("BSERISK{}-{suffix}.XML", trading_date.format("%Y%m%d"));
    Ok(SpanPublication {
        url,
        filename,
        member,
        snapshot,
        required: true,
    })
}

fn public_span_publications(trading_date: NaiveDate) -> Result<Vec<SpanPublication>, DataError> {
    let snapshots = [
        ("00", SnapshotLocator::begin_of_day()),
        ("01", SnapshotLocator::intraday(1)),
        ("02", SnapshotLocator::intraday(2)),
        ("03", SnapshotLocator::intraday(3)),
        ("04", SnapshotLocator::intraday(4)),
        ("FINAL", SnapshotLocator::end_of_day()),
    ];
    let mut publications = Vec::with_capacity(snapshots.len());
    for (suffix, snapshot) in snapshots {
        let filename = format!("BSERISK{}-{suffix}.ZIP", trading_date.format("%Y%m%d"));
        let member = format!("BSERISK{}-{suffix}.XML", trading_date.format("%Y%m%d"));
        let url =
            Url::parse(&format!("{PUBLIC_PREFIX}Risk_Automate/{filename}")).map_err(|_| {
                DataError::DownloadRejected {
                    detail: "ICCL SPAN URL is not valid".to_owned(),
                }
            })?;
        publications.push(SpanPublication {
            url,
            filename,
            member,
            snapshot,
            required: false,
        });
    }
    Ok(publications)
}

fn error_detail(error: &reqwest::Error) -> String {
    let mut detail = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        detail.push_str(": ");
        detail.push_str(&inner.to_string());
        source = std::error::Error::source(inner);
    }
    detail
}

fn snapshot_for_mode(mode: &str) -> Option<(&'static str, SnapshotLocator)> {
    match mode {
        "B" => Some(("00", SnapshotLocator::begin_of_day())),
        "I" => Some(("01", SnapshotLocator::intraday(1))),
        "J" => Some(("02", SnapshotLocator::intraday(2))),
        "K" => Some(("03", SnapshotLocator::intraday(3))),
        "L" => Some(("04", SnapshotLocator::intraday(4))),
        "Z" => Some(("FINAL", SnapshotLocator::end_of_day())),
        _ => None,
    }
}

fn public_span_url(file_path: &str, filename: &str) -> Result<Url, DataError> {
    let Some(rest) = file_path.strip_prefix(NOTICES_PREFIX) else {
        return Err(DataError::DownloadRejected {
            detail: "ICCL SPAN path is not on notices.bseindia.com".to_owned(),
        });
    };
    if rest != format!("Risk_Automate/{filename}") {
        return Err(DataError::DownloadRejected {
            detail: format!("ICCL SPAN path `{file_path}` is not the file for this trading date"),
        });
    }
    Url::parse(&format!("{PUBLIC_PREFIX}{rest}")).map_err(|_| DataError::DownloadRejected {
        detail: "ICCL SPAN URL is not valid".to_owned(),
    })
}

async fn download_bytes(
    client: &reqwest::Client,
    policy: &DownloadPolicy,
    url: &Url,
    filename: &str,
) -> Result<Vec<u8>, DataError> {
    policy.authorize(url)?;
    let response = client
        .get(url.clone())
        .version(reqwest::Version::HTTP_11)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .await
        .map_err(|error| DataError::DownloadRejected {
            detail: format!("ICCL file request failed: {}", error_detail(&error)),
        })?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(DataError::DownloadRejected {
            detail: format!("no official file was published at {filename}"),
        });
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(DataError::DownloadRejected {
            detail: format!("ICCL file request returned HTTP {}", response.status()),
        });
    }
    if response.url().host_str() != Some(ELM_HOST) {
        return Err(DataError::DownloadRejected {
            detail: "ICCL file response left www.bseindia.com".to_owned(),
        });
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !media_type_matches(
        content_type,
        &["application/x-zip-compressed", "application/zip"],
    ) {
        return Err(DataError::DownloadRejected {
            detail: format!("ICCL file content type `{content_type}` is not a zip"),
        });
    }
    read_response_body(response, policy.max_bytes(), "ICCL").await
}

fn day_month_year(trading_date: NaiveDate) -> String {
    format!(
        "{:02}{:02}{:02}",
        trading_date.day(),
        trading_date.month(),
        trading_date.year() % 100
    )
}

struct SpanPublication {
    url: Url,
    filename: String,
    member: String,
    snapshot: SnapshotLocator,
    /// A file named by the listing must exist. A public-path probe may be absent.
    required: bool,
}

#[derive(Deserialize)]
struct Catalog {
    #[serde(rename = "Table")]
    table: Vec<CatalogRow>,
}

#[derive(Deserialize)]
struct CatalogRow {
    #[serde(rename = "File_Mode")]
    file_mode: String,
    #[serde(rename = "File_Path")]
    file_path: String,
    #[serde(rename = "FILE_TYPE")]
    file_type: String,
}

#[cfg(test)]
mod tests {
    use super::{public_span_url, publication};
    use chrono::NaiveDate;

    #[test]
    fn the_page_rewrites_the_notices_path_onto_the_public_host() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 28).expect("date");
        let row = super::CatalogRow {
            file_mode: "L".to_owned(),
            file_path: "http://notices.bseindia.com/Risk_Automate/BSERISK20261028-04.ZIP"
                .to_owned(),
            file_type: "XML_FILE".to_owned(),
        };
        let published = publication(date, &row).expect("publication");
        assert_eq!(
            published.url.as_str(),
            "https://www.bseindia.com/bsedata/Risk_Automate/BSERISK20261028-04.ZIP"
        );
        assert_eq!(published.member, "BSERISK20261028-04.XML");
        assert!(
            public_span_url(
                "http://notices.bseindia.com/Risk_Automate/BSERISK20261027-04.ZIP",
                "BSERISK20261028-04.ZIP"
            )
            .is_err()
        );
        let public = super::public_span_publications(date).expect("public paths");
        let fourth = public
            .iter()
            .find(|row| row.filename.ends_with("-04.ZIP"))
            .expect("intraday 04");
        assert_eq!(fourth.url, published.url);
        assert_eq!(fourth.member, published.member);
        assert!(!fourth.required);
        assert!(published.required);
    }
}
