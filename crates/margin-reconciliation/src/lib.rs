//! Reconciliation arithmetic.
//!
//! A difference can sit inside a diagnostic tolerance and still be material.
//! This crate does not decide that the engine matches a clearing corporation.

mod difference;

pub use difference::{
    CompareInput, DiagnosticCategory, Difference, ReconciliationError, ResolutionStatus, Tolerance,
    compare,
};
