use crate::{DataError, redact::redact};
use margin_domain::{ClearingCorporation, Exchange};
use reqwest::Url;
use std::collections::BTreeSet;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadPolicy {
    approved_hosts: BTreeSet<String>,
    timeout: Duration,
    max_redirects: usize,
    max_bytes: u64,
}

impl DownloadPolicy {
    pub fn new(
        approved_hosts: impl IntoIterator<Item = impl AsRef<str>>,
        timeout: Duration,
        max_redirects: usize,
        max_bytes: u64,
    ) -> Result<Self, DataError> {
        if timeout.is_zero() || max_bytes == 0 {
            return Err(DataError::Configuration {
                detail: "download timeout and max bytes must be positive".to_owned(),
            });
        }
        let mut hosts = BTreeSet::new();
        for host in approved_hosts {
            hosts.insert(normalize_host(host.as_ref())?);
        }
        Ok(Self {
            approved_hosts: hosts,
            timeout,
            max_redirects,
            max_bytes,
        })
    }

    pub fn approved_hosts(&self) -> &BTreeSet<String> {
        &self.approved_hosts
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn max_redirects(&self) -> usize {
        self.max_redirects
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn authorize(&self, url: &Url) -> Result<(), DataError> {
        authorize_url(url, &self.approved_hosts)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfiguredSource {
    id: String,
    enabled: bool,
    exchange: Exchange,
    clearing_corporation: ClearingCorporation,
    url: Option<Url>,
    blocker: Option<String>,
}

impl ConfiguredSource {
    pub fn new(
        id: impl Into<String>,
        enabled: bool,
        exchange: Exchange,
        clearing_corporation: ClearingCorporation,
        url: Option<Url>,
        blocker: Option<String>,
    ) -> Result<Self, DataError> {
        let id = id.into();
        if id.trim().is_empty() || id.len() > 64 {
            return Err(DataError::Configuration {
                detail: "source id must be 1 to 64 characters".to_owned(),
            });
        }
        if enabled {
            if url.is_none() {
                return Err(DataError::Configuration {
                    detail: format!("enabled source `{id}` requires a URL"),
                });
            }
            if blocker
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Err(DataError::Configuration {
                    detail: format!("enabled source `{id}` cannot carry a blocker"),
                });
            }
        } else if blocker.as_ref().is_none_or(|value| value.trim().is_empty()) {
            return Err(DataError::Configuration {
                detail: format!("disabled source `{id}` requires a blocker"),
            });
        }
        Ok(Self {
            id,
            enabled,
            exchange,
            clearing_corporation,
            url,
            blocker,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn exchange(&self) -> Exchange {
        self.exchange
    }

    pub fn clearing_corporation(&self) -> ClearingCorporation {
        self.clearing_corporation
    }

    pub fn url(&self) -> Option<&Url> {
        self.url.as_ref()
    }

    pub fn blocker(&self) -> Option<&str> {
        self.blocker.as_deref()
    }
}

pub fn ensure_source_fetchable(
    source: &ConfiguredSource,
    policy: &DownloadPolicy,
) -> Result<(), DataError> {
    if !source.enabled {
        return Err(DataError::SourceDisabled {
            id: source.id.clone(),
            blocker: source
                .blocker
                .clone()
                .unwrap_or_else(|| "source is disabled".to_owned()),
        });
    }
    let Some(url) = source.url.as_ref() else {
        return Err(DataError::DownloadRejected {
            detail: format!("source `{}` has no URL", source.id),
        });
    };
    policy.authorize(url)
}

pub fn build_download_client(policy: &DownloadPolicy) -> Result<reqwest::Client, DataError> {
    let hosts = policy.approved_hosts.clone();
    let max_redirects = policy.max_redirects;
    reqwest::Client::builder()
        .timeout(policy.timeout)
        .cookie_store(true)
        // BSE's extreme-loss response folds a header with a leading space.
        // Hyper rejects that line. The file body is unchanged.
        .http1_ignore_invalid_headers_in_responses(true)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= max_redirects {
                return attempt.error(DataError::DownloadRejected {
                    detail: "redirect limit exceeded".to_owned(),
                });
            }
            match authorize_url(attempt.url(), &hosts) {
                Ok(()) => attempt.follow(),
                Err(error) => attempt.error(error),
            }
        }))
        .build()
        .map_err(|error| DataError::Configuration {
            detail: format!(
                "download client could not be built: {}",
                redact(&error.to_string(), &[])
            ),
        })
}

fn authorize_url(url: &Url, approved_hosts: &BTreeSet<String>) -> Result<(), DataError> {
    if url.scheme() != "https" {
        return Err(DataError::DownloadRejected {
            detail: "only HTTPS downloads are permitted".to_owned(),
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DataError::DownloadRejected {
            detail: "download URLs cannot contain userinfo".to_owned(),
        });
    }
    if let Some(port) = url.port()
        && port != 443
    {
        return Err(DataError::DownloadRejected {
            detail: "download URLs must use port 443".to_owned(),
        });
    }
    let host = url
        .host_str()
        .ok_or_else(|| DataError::DownloadRejected {
            detail: "download URL is missing a host".to_owned(),
        })?
        .to_ascii_lowercase();
    if !approved_hosts.contains(&host) {
        return Err(DataError::DownloadRejected {
            detail: format!("host `{host}` is not on the approved HTTPS host list"),
        });
    }
    Ok(())
}

pub(crate) fn media_type_matches(header: &str, allowed: &[&str]) -> bool {
    let media = header
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    allowed.iter().any(|expected| media == *expected)
}

pub(crate) fn file_not_published(error: &DataError) -> bool {
    matches!(
        error,
        DataError::DownloadRejected { detail } if detail.contains("no official file was published")
    )
}

pub(crate) async fn read_response_body(
    mut response: reqwest::Response,
    max_bytes: u64,
    source: &str,
) -> Result<Vec<u8>, DataError> {
    if let Some(length) = response.content_length() {
        crate::hash::enforce_size_limit(length, max_bytes)?;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| DataError::DownloadRejected {
            detail: format!("{source} response body could not be read: {error}"),
        })?
    {
        let next = u64::try_from(body.len().saturating_add(chunk.len())).unwrap_or(u64::MAX);
        if next > max_bytes {
            return Err(DataError::ArchiveRejected {
                detail: format!("payload exceeds the configured limit of {max_bytes} bytes"),
            });
        }
        body.extend_from_slice(&chunk);
    }
    if body.is_empty() {
        return Err(DataError::DownloadRejected {
            detail: format!("{source} response body is empty"),
        });
    }
    Ok(body)
}

fn normalize_host(host: &str) -> Result<String, DataError> {
    let normalized = host.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.contains('/')
        || normalized.contains(':')
        || normalized.contains(' ')
    {
        return Err(DataError::Configuration {
            detail: format!("approved host `{host}` is not a bare hostname"),
        });
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::{ConfiguredSource, DownloadPolicy, build_download_client, ensure_source_fetchable};
    use margin_domain::{ClearingCorporation, Exchange};
    use reqwest::Url;
    use std::time::Duration;

    fn policy() -> DownloadPolicy {
        DownloadPolicy::new(["www.nseclearing.in"], Duration::from_secs(5), 1, 1024)
            .expect("policy")
    }

    #[test]
    fn only_approved_https_hosts_are_fetchable() {
        let policy = policy();
        let http = Url::parse("http://www.nseclearing.in/file").expect("url");
        let foreign = Url::parse("https://example.com/file").expect("url");
        let approved = Url::parse("https://www.nseclearing.in/file").expect("url");
        assert!(policy.authorize(&http).is_err());
        assert!(policy.authorize(&foreign).is_err());
        assert!(policy.authorize(&approved).is_ok());
        assert!(build_download_client(&policy).is_ok());

        let source = ConfiguredSource::new(
            "nse-clearing",
            false,
            Exchange::Nse,
            ClearingCorporation::Nscc,
            Some(approved),
            Some("format not verified".to_owned()),
        )
        .expect("source");
        assert!(ensure_source_fetchable(&source, &policy).is_err());
    }
}
