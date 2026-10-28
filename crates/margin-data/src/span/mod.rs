//! SPAN 4.00 parsing and the quote shared by NSCCL and ICCL files.

pub(crate) mod quote;
pub(crate) mod xml;

pub use quote::{
    CashMarginRates, ContractElmRates, ExposureRates, MarginLine, MarginPosition, SpanMarginQuote,
    canonical_contract_symbol, check_margin_positions, quote_span_zip,
};
