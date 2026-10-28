//! SPAN components.
//!
//! [`diagnostic`] implements the portfolio P&L sum specified for this project.
//! That result is a diagnostic. It is not an NSE Clearing or ICCL SPAN margin.
//! Official scanning risk, spreads, short-option minimums, and net option value
//! fail closed until a verified methodology is implemented.

mod diagnostic;
mod official;

pub use diagnostic::{
    DIAGNOSTIC_LIMITATION, DiagnosticAggregation, ScenarioDiagnostic, aggregate_scenarios,
};
pub use official::{
    SpanError, diagnostic_is_not_scanning_risk, net_option_value, scanning_risk,
    short_option_minimum, spread_charge,
};
