use crate::diagnostic::DiagnosticAggregation;
use margin_domain::{DomainError, Money, NormalizedBook, ScenarioLayoutRegistry};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpanError {
    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("official SPAN component `{component}` is not implemented: {detail}")]
    SpecificationRequired {
        component: &'static str,
        detail: &'static str,
    },
}

fn not_implemented(component: &'static str, detail: &'static str) -> SpanError {
    SpanError::SpecificationRequired { component, detail }
}

/// Official scanning risk. A registered layout does not unlock a formula.
pub fn scanning_risk(
    registry: &ScenarioLayoutRegistry,
    format_id: &str,
    parser_version: &str,
    book: &NormalizedBook,
) -> Result<Money, SpanError> {
    let _ = book;
    if !registry.is_registered(format_id, parser_version) {
        return Err(SpanError::Domain(DomainError::MissingRequiredData {
            detail: format!(
                "no scenario layout specification is registered for format `{format_id}` parser `{parser_version}`"
            ),
        }));
    }
    Err(not_implemented(
        "scanning_risk",
        "Official NSE Clearing and ICCL scanning-risk methodology is not implemented.",
    ))
}

pub fn spread_charge(book: &NormalizedBook) -> Result<Money, SpanError> {
    let _ = book;
    Err(not_implemented(
        "spread_charge",
        "Official spread eligibility, units, and charge schedules are not implemented.",
    ))
}

pub fn short_option_minimum(book: &NormalizedBook) -> Result<Money, SpanError> {
    let _ = book;
    Err(not_implemented(
        "short_option_minimum",
        "Official short-option minimum rules are not implemented.",
    ))
}

pub fn net_option_value(book: &NormalizedBook) -> Result<Money, SpanError> {
    let _ = book;
    Err(not_implemented(
        "net_option_value",
        "Official option valuation and net-option-value rules are not implemented.",
    ))
}

/// Prevents a diagnostic worst loss from being treated as scanning risk.
pub fn diagnostic_is_not_scanning_risk(diagnostic: &DiagnosticAggregation) -> bool {
    diagnostic.limitation == crate::diagnostic::DIAGNOSTIC_LIMITATION
}
