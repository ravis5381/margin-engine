//! NSE reports that clear at NSCCL.
//!
//! The exchange is NSE. The clearing corporation on these files is NSCCL.

pub(crate) mod contract_master;
pub(crate) mod fetch;
pub(crate) mod ingest;
pub(crate) mod quote;
pub(crate) mod reports;

pub use contract_master::ContractMaster;
pub use ingest::{ensure_nse_margin_files, ingest_nse_report, verified_nse_source};
pub use reports::NseReportKind;
