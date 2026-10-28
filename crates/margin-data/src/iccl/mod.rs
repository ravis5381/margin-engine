//! BSE equity-derivatives files that clear at ICCL.

pub(crate) mod ingest;
pub(crate) mod quote;

pub use ingest::{ensure_iccl_margin_files, ingest_iccl_files};
