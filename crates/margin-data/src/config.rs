use crate::{ConfiguredSource, DataError, DownloadPolicy};
use chrono::NaiveDate;
use margin_domain::{ClearingArrangement, ClearingConfiguration, ClearingCorporation, Exchange};
use serde::Deserialize;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Text,
}

impl LogFormat {
    pub fn parse(value: &str) -> Result<Self, DataError> {
        match value {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            other => Err(DataError::Configuration {
                detail: format!("log format `{other}` must be json or text"),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeEnv {
    pub database_url: Option<String>,
    pub bind_addr: Option<String>,
    pub log_format: Option<String>,
}

pub enum ConfigFile {
    Absent,
    Contents(String),
}

#[derive(Clone)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub request_body_limit_bytes: usize,
    pub request_timeout: Duration,
    pub max_concurrency: usize,
    pub database_url: Option<String>,
    pub max_db_connections: u32,
    pub db_acquire_timeout: Duration,
    pub log_format: LogFormat,
    pub max_file_bytes: u64,
    pub download: DownloadPolicy,
    pub sources: Vec<ConfiguredSource>,
    pub clearing: ClearingConfiguration,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("bind", &self.bind)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "[redacted]"),
            )
            .field("log_format", &self.log_format)
            .field("source_count", &self.sources.len())
            .field("clearing_arrangements", &self.clearing.arrangements().len())
            .finish()
    }
}

impl AppConfig {
    pub fn database_url(&self) -> Option<&str> {
        self.database_url.as_deref()
    }
}

pub fn assemble(
    app: ConfigFile,
    sources: ConfigFile,
    env: RuntimeEnv,
) -> Result<AppConfig, DataError> {
    let mut parsed = match app {
        ConfigFile::Absent => default_parts(),
        ConfigFile::Contents(contents) => parse_app(&contents)?,
    };
    match sources {
        ConfigFile::Absent => {
            parsed.download_hosts.clear();
            parsed.sources.clear();
        }
        ConfigFile::Contents(contents) => {
            let loaded = parse_sources(&contents)?;
            parsed.download_hosts = loaded.0;
            parsed.sources = loaded.1;
        }
    }
    if let Some(bind) = env.bind_addr.as_deref() {
        parsed.bind = parse_bind(bind)?;
    }
    if let Some(format) = env.log_format.as_deref() {
        parsed.log_format = LogFormat::parse(format)?;
    }
    let database_url = match env.database_url.as_deref() {
        Some(url) => {
            validate_database_url(url)?;
            Some(url.to_owned())
        }
        None => None,
    };
    let download = DownloadPolicy::new(
        parsed.download_hosts,
        parsed.download_timeout,
        parsed.max_redirects,
        parsed.max_file_bytes,
    )?;
    for source in &parsed.sources {
        if let Some(url) = source.url() {
            download.authorize(url)?;
        }
    }
    Ok(AppConfig {
        bind: parsed.bind,
        request_body_limit_bytes: parsed.request_body_limit_bytes,
        request_timeout: parsed.request_timeout,
        max_concurrency: parsed.max_concurrency,
        database_url,
        max_db_connections: parsed.max_db_connections,
        db_acquire_timeout: parsed.db_acquire_timeout,
        log_format: parsed.log_format,
        max_file_bytes: parsed.max_file_bytes,
        download,
        sources: parsed.sources,
        clearing: parsed.clearing,
    })
}

struct ParsedApp {
    bind: SocketAddr,
    request_body_limit_bytes: usize,
    request_timeout: Duration,
    max_concurrency: usize,
    max_db_connections: u32,
    db_acquire_timeout: Duration,
    log_format: LogFormat,
    max_file_bytes: u64,
    download_timeout: Duration,
    max_redirects: usize,
    download_hosts: Vec<String>,
    sources: Vec<ConfiguredSource>,
    clearing: ClearingConfiguration,
}

fn default_parts() -> ParsedApp {
    ParsedApp {
        bind: "127.0.0.1:8080".parse().expect("static bind address"),
        request_body_limit_bytes: 1_048_576,
        request_timeout: Duration::from_secs(30),
        max_concurrency: 64,
        max_db_connections: 5,
        db_acquire_timeout: Duration::from_secs(5),
        log_format: LogFormat::Json,
        max_file_bytes: 104_857_600,
        download_timeout: Duration::from_secs(30),
        max_redirects: 2,
        download_hosts: Vec::new(),
        sources: Vec::new(),
        clearing: ClearingConfiguration::empty(),
    }
}

fn parse_app(contents: &str) -> Result<ParsedApp, DataError> {
    let raw: RawApp = toml::from_str(contents).map_err(|error| DataError::Configuration {
        detail: format!("application config could not be parsed: {error}"),
    })?;
    let mut arrangements = Vec::with_capacity(raw.clearing_arrangements.len());
    for arrangement in raw.clearing_arrangements {
        let effective_from = parse_date(&arrangement.effective_from)?;
        let effective_to = match arrangement.effective_to.as_deref() {
            Some(value) => Some(parse_date(value)?),
            None => None,
        };
        arrangements.push(ClearingArrangement::try_new(
            Exchange::parse(&arrangement.exchange)?,
            ClearingCorporation::parse(&arrangement.clearing_corporation)?,
            effective_from,
            effective_to,
            arrangement.cross_exchange_offset_permitted,
            arrangement.rule_reference,
        )?);
    }
    Ok(ParsedApp {
        bind: parse_bind(&raw.server.bind)?,
        request_body_limit_bytes: checked_range(
            raw.server.request_body_limit_bytes,
            1_024,
            10_485_760,
            "request body limit",
        )? as usize,
        request_timeout: Duration::from_secs(checked_range(
            raw.server.request_timeout_secs,
            1,
            300,
            "request timeout",
        )?),
        max_concurrency: checked_range(
            raw.server.max_concurrency as u64,
            1,
            256,
            "max concurrency",
        )? as usize,
        max_db_connections: checked_range(
            u64::from(raw.database.max_connections),
            1,
            32,
            "database pool size",
        )? as u32,
        db_acquire_timeout: Duration::from_secs(checked_range(
            raw.database.acquire_timeout_secs,
            1,
            30,
            "database acquire timeout",
        )?),
        log_format: LogFormat::parse(&raw.logging.format)?,
        max_file_bytes: checked_range(
            raw.archive.max_file_bytes,
            1_024,
            536_870_912,
            "archive limit",
        )?,
        download_timeout: Duration::from_secs(checked_range(
            raw.download.timeout_secs,
            1,
            120,
            "download timeout",
        )?),
        max_redirects: checked_range(raw.download.max_redirects, 0, 5, "max redirects")? as usize,
        download_hosts: Vec::new(),
        sources: Vec::new(),
        clearing: ClearingConfiguration::try_from_arrangements(arrangements)?,
    })
}

fn parse_sources(contents: &str) -> Result<(Vec<String>, Vec<ConfiguredSource>), DataError> {
    let raw: RawSources = toml::from_str(contents).map_err(|error| DataError::Configuration {
        detail: format!("data source config could not be parsed: {error}"),
    })?;
    let hosts: Vec<String> = raw
        .approved_hosts
        .into_iter()
        .map(|host| host.host)
        .collect();
    let mut sources = Vec::with_capacity(raw.sources.len());
    for source in raw.sources {
        let url = match source.url.as_deref() {
            Some(value) => Some(Url::parse(value).map_err(|_| DataError::Configuration {
                detail: format!("source `{}` URL is invalid", source.id),
            })?),
            None => None,
        };
        sources.push(ConfiguredSource::new(
            source.id,
            source.enabled,
            Exchange::parse(&source.exchange)?,
            ClearingCorporation::parse(&source.clearing_corporation)?,
            url,
            source.blocker,
        )?);
    }
    let mut ids = Vec::new();
    for source in &sources {
        if ids.contains(&source.id()) {
            return Err(DataError::Configuration {
                detail: format!("source `{}` is duplicated", source.id()),
            });
        }
        ids.push(source.id());
    }
    Ok((hosts, sources))
}

fn parse_bind(value: &str) -> Result<SocketAddr, DataError> {
    value.parse().map_err(|_| DataError::Configuration {
        detail: "bind address is invalid".to_owned(),
    })
}

fn parse_date(value: &str) -> Result<NaiveDate, DataError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| DataError::Configuration {
        detail: format!("date `{value}` must use YYYY-MM-DD"),
    })
}

fn validate_database_url(url: &str) -> Result<(), DataError> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("postgres://") || lower.starts_with("postgresql://")) {
        return Err(DataError::Configuration {
            detail: "DATABASE_URL must start with postgres:// or postgresql://".to_owned(),
        });
    }
    if url.chars().any(char::is_whitespace) {
        return Err(DataError::Configuration {
            detail: "DATABASE_URL cannot contain whitespace".to_owned(),
        });
    }
    Ok(())
}

fn checked_range(value: u64, min: u64, max: u64, what: &str) -> Result<u64, DataError> {
    if value < min || value > max {
        return Err(DataError::Configuration {
            detail: format!("{what} must be between {min} and {max}"),
        });
    }
    Ok(value)
}

#[derive(Debug, Deserialize)]
struct RawApp {
    server: RawServer,
    database: RawDatabase,
    logging: RawLogging,
    archive: RawArchive,
    download: RawDownload,
    #[serde(default)]
    clearing_arrangements: Vec<RawArrangement>,
}

#[derive(Debug, Deserialize)]
struct RawServer {
    bind: String,
    request_body_limit_bytes: u64,
    request_timeout_secs: u64,
    max_concurrency: u64,
}

#[derive(Debug, Deserialize)]
struct RawDatabase {
    max_connections: u32,
    acquire_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
struct RawLogging {
    format: String,
}

#[derive(Debug, Deserialize)]
struct RawArchive {
    max_file_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct RawDownload {
    timeout_secs: u64,
    max_redirects: u64,
}

#[derive(Debug, Deserialize)]
struct RawArrangement {
    exchange: String,
    clearing_corporation: String,
    effective_from: String,
    #[serde(default)]
    effective_to: Option<String>,
    cross_exchange_offset_permitted: bool,
    rule_reference: String,
}

#[derive(Debug, Deserialize)]
struct RawSources {
    #[serde(default)]
    approved_hosts: Vec<RawHost>,
    #[serde(default)]
    sources: Vec<RawSource>,
}

#[derive(Debug, Deserialize)]
struct RawHost {
    host: String,
}

#[derive(Debug, Deserialize)]
struct RawSource {
    id: String,
    enabled: bool,
    exchange: String,
    clearing_corporation: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    blocker: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{ConfigFile, RuntimeEnv, assemble};
    use crate::ensure_source_fetchable;
    use chrono::NaiveDate;
    use margin_domain::{ClearingCorporation, Exchange};

    fn env(database_url: Option<&str>) -> RuntimeEnv {
        RuntimeEnv {
            database_url: database_url.map(str::to_owned),
            bind_addr: None,
            log_format: None,
        }
    }

    #[test]
    fn missing_files_authorize_nothing_and_redact_the_database_url() {
        let config = assemble(ConfigFile::Absent, ConfigFile::Absent, env(None)).expect("defaults");
        let date = NaiveDate::from_ymd_opt(2026, 10, 28).expect("date");
        assert!(
            config
                .clearing
                .authorize(Exchange::Nse, ClearingCorporation::Nscc, date)
                .is_err()
        );

        let configured = assemble(
            ConfigFile::Absent,
            ConfigFile::Absent,
            env(Some("postgres://margin:change-me@localhost/margin_engine")),
        )
        .expect("config");
        let rendered = format!("{configured:?}");
        assert!(!rendered.contains("change-me"));
        assert!(rendered.contains("[redacted]"));
    }

    #[test]
    fn example_files_enable_the_verified_nse_source_and_keep_iccl_disabled() {
        let config = assemble(
            ConfigFile::Contents(include_str!("../../../config/app.example.toml").to_owned()),
            ConfigFile::Contents(
                include_str!("../../../config/data_sources.example.toml").to_owned(),
            ),
            env(None),
        )
        .expect("examples");
        let date = NaiveDate::from_ymd_opt(2026, 10, 28).expect("date");
        assert!(
            config
                .clearing
                .arrangements()
                .iter()
                .all(|arrangement| !arrangement.cross_exchange_offset_permitted())
        );
        assert!(
            config
                .clearing
                .authorize(Exchange::Nse, ClearingCorporation::Nscc, date)
                .is_ok()
        );
        assert!(
            config
                .clearing
                .authorize(Exchange::Bse, ClearingCorporation::Nscc, date)
                .is_err()
        );
        let nse = config
            .sources
            .iter()
            .find(|source| source.id() == "nse-clearing-equity-derivatives")
            .expect("nse source");
        let iccl = config
            .sources
            .iter()
            .find(|source| source.id() == "iccl-equity-derivatives-risk-parameters")
            .expect("iccl source");
        assert!(nse.enabled());
        assert!(ensure_source_fetchable(nse, &config.download).is_ok());
        assert!(!iccl.enabled());
        assert!(ensure_source_fetchable(iccl, &config.download).is_err());
    }
}
