//! SPAN scanning risk, extreme loss margin, and option premium.
//!
//! NSE Clearing states that a risk-array loss is positive and a gain is negative
//! for a long unit. Scanning risk is the greatest scenario loss of the book.
//! Monthly symbols use `YY` and `MON`. Dated symbols use `YY`, the month code
//! `1`–`9`, `O`, `N`, or `D`, and `DD`. A physical stock is the product code.
//! Stock membership comes from the exposure file. Extreme-loss percents come
//! from the contract-wise ELM file when that file is loaded.

mod extract;
mod file;
mod inputs;
mod price;
mod rates;
mod symbol;

#[cfg(test)]
mod tests;

pub(crate) use extract::{CommodityFamily, SpanFileIndex, SpreadFault, parse_span_index};
pub(crate) use file::{CalendarSpread, Commodity, ContractKind, SpanContract};
pub(crate) use inputs::QuoteBuilder;
pub use price::{MarginLine, SpanMarginQuote};
pub use rates::{CashMarginRates, ContractElmRates, ExposureRates};
pub use symbol::{MarginPosition, canonical_contract_symbol, check_margin_positions};

use chrono::NaiveDate;

pub fn quote_span_zip(
    bytes: &[u8],
    trading_date: NaiveDate,
    positions: &[MarginPosition],
    exposure: &ExposureRates,
    contracts: &ContractElmRates,
    cash: &CashMarginRates,
) -> Result<SpanMarginQuote, String> {
    let xml = file::span_xml_bytes(bytes)?;
    quote_span_xml(&xml, trading_date, positions, exposure, contracts, cash)
}

pub(crate) fn span_index_from_zip(bytes: &[u8]) -> Result<SpanFileIndex, String> {
    parse_span_index(&file::span_xml_bytes(bytes)?)
}

pub fn quote_span_xml(
    bytes: &[u8],
    trading_date: NaiveDate,
    positions: &[MarginPosition],
    exposure: &ExposureRates,
    contracts: &ContractElmRates,
    cash: &CashMarginRates,
) -> Result<SpanMarginQuote, String> {
    QuoteBuilder::on(trading_date, positions)
        .exposure(exposure)
        .contracts(contracts)
        .cash(cash)
        .price_xml(bytes)
}
