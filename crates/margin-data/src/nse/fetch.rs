use crate::download::read_response_body;
use crate::nse::reports::{
    NSE_HOST, NSE_REFERER, NSE_REPORTS_PATH, NSE_SESSION_REFERER, NSE_SESSION_URL, NSE_USER_AGENT,
    NSE_VAR_HOST, NseReportKind, cash_var_url, content_disposition_filename, contract_master_url,
    media_type_matches, reports_url,
};
use crate::{DataError, DownloadPolicy};
use chrono::NaiveDate;
use reqwest::header::{ACCEPT, CONTENT_DISPOSITION, CONTENT_TYPE, REFERER, USER_AGENT};
use reqwest::{StatusCode, Url};

const SESSION_BODY_LIMIT: u64 = 5_242_880;

pub(crate) struct DownloadedReport {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub source_host: String,
}

pub(crate) async fn download_nse_report(
    client: &reqwest::Client,
    policy: &DownloadPolicy,
    kind: NseReportKind,
    trading_date: NaiveDate,
) -> Result<DownloadedReport, DataError> {
    let session = Url::parse(NSE_SESSION_URL).map_err(|_| DataError::DownloadRejected {
        detail: "NSE session URL is not valid".to_owned(),
    })?;
    policy.authorize(&session)?;
    let reports = reports_url(kind, trading_date)?;
    policy.authorize(&reports)?;

    let session_response = client
        .get(session)
        .header(USER_AGENT, NSE_USER_AGENT)
        .header(ACCEPT, "text/html")
        .header(REFERER, NSE_SESSION_REFERER)
        .send()
        .await
        .map_err(|error| DataError::DownloadRejected {
            detail: format!("NSE session request failed: {error}"),
        })?;
    if session_response.status() != StatusCode::OK {
        return Err(DataError::DownloadRejected {
            detail: format!(
                "NSE session request returned HTTP {}",
                session_response.status()
            ),
        });
    }
    if session_response.url().host_str() != Some(NSE_HOST) {
        return Err(DataError::DownloadRejected {
            detail: "NSE session response left the approved NSE host".to_owned(),
        });
    }
    let _session_body = read_response_body(
        session_response,
        SESSION_BODY_LIMIT.min(policy.max_bytes()),
        "NSE",
    )
    .await?;

    let response = client
        .get(reports)
        .header(USER_AGENT, NSE_USER_AGENT)
        .header(REFERER, NSE_REFERER)
        .header(ACCEPT, "*/*")
        .send()
        .await
        .map_err(|error| DataError::DownloadRejected {
            detail: format!("NSE report request failed: {error}"),
        })?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(DataError::DownloadRejected {
            detail: format!(
                "no official file was published for {} on {}",
                kind.code(),
                trading_date
            ),
        });
    }
    if status != StatusCode::OK {
        return Err(DataError::DownloadRejected {
            detail: format!("NSE report request returned HTTP {status}"),
        });
    }
    let final_url = response.url().clone();
    if final_url.host_str() != Some(NSE_HOST) || !final_url.path().starts_with(NSE_REPORTS_PATH) {
        return Err(DataError::DownloadRejected {
            detail: "NSE report response did not stay on the verified reports path".to_owned(),
        });
    }
    let content_type = header_text(&response, CONTENT_TYPE)?;
    if !media_type_matches(&content_type, kind.expected_media_type()) {
        return Err(DataError::DownloadRejected {
            detail: format!(
                "NSE report content type `{content_type}` did not match the verified type for {}",
                kind.code()
            ),
        });
    }
    let disposition = header_text(&response, CONTENT_DISPOSITION)?;
    let filename = content_disposition_filename(&disposition)?;
    let expected = kind.expected_filename(trading_date)?;
    if filename != expected {
        return Err(DataError::DownloadRejected {
            detail: format!("NSE report filename `{filename}` did not match `{expected}`"),
        });
    }
    let bytes = read_response_body(response, policy.max_bytes(), "NSE").await?;
    Ok(DownloadedReport {
        filename,
        bytes,
        source_host: NSE_HOST.to_owned(),
    })
}

pub(crate) async fn download_contract_master(
    client: &reqwest::Client,
    policy: &DownloadPolicy,
    trading_date: NaiveDate,
) -> Result<DownloadedReport, DataError> {
    let url = contract_master_url(trading_date)?;
    policy.authorize(&url)?;
    let filename = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| DataError::DownloadRejected {
            detail: "contract master URL has no filename".to_owned(),
        })?
        .to_owned();
    let response = client
        .get(url)
        .header(USER_AGENT, NSE_USER_AGENT)
        .header(ACCEPT, "*/*")
        .send()
        .await
        .map_err(|error| DataError::DownloadRejected {
            detail: format!("contract master request failed: {error}"),
        })?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(DataError::DownloadRejected {
            detail: format!(
                "no official file was published for the contract master on {trading_date}"
            ),
        });
    }
    if status != StatusCode::OK {
        return Err(DataError::DownloadRejected {
            detail: format!("contract master request returned HTTP {status}"),
        });
    }
    if response.url().host_str() != Some(NSE_VAR_HOST) {
        return Err(DataError::DownloadRejected {
            detail: "contract master response left the approved NSE archive host".to_owned(),
        });
    }
    let bytes = read_response_body(response, policy.max_bytes(), "NSE").await?;
    Ok(DownloadedReport {
        filename,
        bytes,
        source_host: NSE_VAR_HOST.to_owned(),
    })
}

pub(crate) async fn download_cash_var(
    client: &reqwest::Client,
    policy: &DownloadPolicy,
    trading_date: NaiveDate,
    batch: u8,
) -> Result<DownloadedReport, DataError> {
    let url = cash_var_url(trading_date, batch)?;
    policy.authorize(&url)?;
    let filename = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| DataError::DownloadRejected {
            detail: "cash-market VaR URL has no filename".to_owned(),
        })?
        .to_owned();
    let response = client
        .get(url)
        .header(USER_AGENT, NSE_USER_AGENT)
        .header(ACCEPT, "*/*")
        .send()
        .await
        .map_err(|error| DataError::DownloadRejected {
            detail: format!("cash-market VaR request failed: {error}"),
        })?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(DataError::DownloadRejected {
            detail: format!(
                "no official file was published for cash-var batch {batch} on {trading_date}"
            ),
        });
    }
    if status != StatusCode::OK {
        return Err(DataError::DownloadRejected {
            detail: format!("cash-market VaR request returned HTTP {status}"),
        });
    }
    if response.url().host_str() != Some(NSE_VAR_HOST) {
        return Err(DataError::DownloadRejected {
            detail: "cash-market VaR response left the NSE archive host".to_owned(),
        });
    }
    let bytes = read_response_body(response, policy.max_bytes(), "NSE").await?;
    Ok(DownloadedReport {
        filename,
        bytes,
        source_host: NSE_VAR_HOST.to_owned(),
    })
}

fn header_text(
    response: &reqwest::Response,
    name: reqwest::header::HeaderName,
) -> Result<String, DataError> {
    let value = response
        .headers()
        .get(&name)
        .ok_or_else(|| DataError::DownloadRejected {
            detail: format!("NSE response is missing the {name} header"),
        })?;
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| DataError::DownloadRejected {
            detail: format!("NSE {name} header is not valid text"),
        })
}
