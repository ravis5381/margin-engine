//! Official risk-file download and archival for NSCCL and ICCL.
//!
//! NSE files clear at NSCCL. BSE SENSEX files clear at ICCL. Both use the shared
//! SPAN quote. Downloads require an enabled source whose HTTPS host is on the
//! allowlist.

mod archive;
mod config;
mod download;
mod error;
mod hash;
mod iccl;
mod nse;
mod postgres;
mod redact;
mod span;
mod store;
mod zip_safe;

pub use archive::IngestReceipt;
pub use config::{AppConfig, ConfigFile, LogFormat, RuntimeEnv, assemble};
pub use download::{
    ConfiguredSource, DownloadPolicy, build_download_client, ensure_source_fetchable,
};
pub use error::DataError;
pub use hash::{enforce_size_limit, sha256_hex};
pub use iccl::{ensure_iccl_margin_files, ingest_iccl_files};
pub use nse::{
    ContractMaster, NseReportKind, ensure_nse_margin_files, ingest_nse_report, verified_nse_source,
};
pub use postgres::{PgStore, StoredSpanQuote, connect, migrate};
pub use redact::{redact as redact_text, sanitize_audit_fields};
pub use span::{
    CashMarginRates, ContractElmRates, ExposureRates, MarginLine, MarginPosition, SpanMarginQuote,
    canonical_contract_symbol, check_margin_positions, quote_span_zip,
};
pub use store::{AppStore, DatasetPage, DatasetReadModel, DatasetSummary, MemoryStore, summarize};
pub use zip_safe::{ZipEntry, ZipLimits, entry_name_allowed, extract_zip_to_memory};
