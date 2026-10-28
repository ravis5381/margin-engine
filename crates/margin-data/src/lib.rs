//! Official NSE report download and archival.
//!
//! Verified NSE files are stored as original bytes. A margin quote reads futures,
//! option, and physical-stock risk arrays from an archived SPAN zip, stock
//! membership from the exposure file, extreme-loss percents from the
//! contract-wise ELM file, and the cash-market daily margin rate from an
//! archived VaR file. The ICCL adapter stays disabled
//! until a file response is captured. Downloads require an enabled source whose
//! HTTPS host is on the allowlist.

mod archive;
mod config;
mod contract_master;
mod download;
mod error;
mod fetch;
mod hash;
mod ingest;
mod nse_reports;
mod postgres;
mod redact;
mod span_quote;
mod span_xml;
mod store;
mod zip_safe;

pub use archive::IngestReceipt;
pub use config::{AppConfig, ConfigFile, LogFormat, RuntimeEnv, assemble};
pub use contract_master::ContractMaster;
pub use download::{
    ConfiguredSource, DownloadPolicy, build_download_client, ensure_source_fetchable,
};
pub use error::DataError;
pub use hash::{enforce_size_limit, sha256_hex};
pub use ingest::{ensure_nse_margin_files, ingest_nse_report, verified_nse_source};
pub use nse_reports::NseReportKind;
pub use postgres::{PgStore, StoredSpanQuote, connect, migrate};
pub use redact::{redact as redact_text, sanitize_audit_fields};
pub use span_quote::{
    CashMarginRates, ContractElmRates, ExposureRates, MarginLine, MarginPosition, SpanMarginQuote,
    canonical_contract_symbol, check_margin_positions, quote_span_zip,
};
pub use store::{AppStore, DatasetPage, DatasetReadModel, DatasetSummary, MemoryStore, summarize};
pub use zip_safe::{ZipEntry, ZipLimits, entry_name_allowed, extract_zip_to_memory};
