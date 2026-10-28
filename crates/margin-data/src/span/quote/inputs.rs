//! The trade date, positions, and rate files for one quote.
//!
//! A rate file left off the builder is not loaded. Exposure and cash then
//! contribute nothing, and the contract file does not replace a published percent.

use super::extract::{self, SpanFileIndex};
use super::file::Book;
use super::price::{self, SpanMarginQuote};
use super::rates::{CashMarginRates, ContractElmRates, ExposureRates};
use super::symbol::{MarginPosition, SymbolSpec, parse_positions};
use chrono::NaiveDate;

#[derive(Clone, Copy)]
pub(super) struct QuoteInputs<'a> {
    pub(super) trading_date: NaiveDate,
    pub(super) positions: &'a [MarginPosition],
    pub(super) exposure: &'a ExposureRates,
    pub(super) contracts: &'a ContractElmRates,
    pub(super) cash: &'a CashMarginRates,
}

impl QuoteInputs<'_> {
    fn price(
        self,
        load: impl FnOnce(&[SymbolSpec]) -> Result<Book, String>,
    ) -> Result<SpanMarginQuote, String> {
        let specs = parse_positions(self.positions)?;
        let specs = self.contracts.identify(self.positions, specs)?;
        let book = load(&specs)?;
        price::quote_book(&book, self, &specs)
    }
}

/// Collects a quote, then prices it from SPAN XML or from a saved index.
pub(crate) struct QuoteBuilder<'a> {
    trading_date: NaiveDate,
    positions: &'a [MarginPosition],
    exposure: Option<&'a ExposureRates>,
    contracts: Option<&'a ContractElmRates>,
    cash: Option<&'a CashMarginRates>,
}

impl<'a> QuoteBuilder<'a> {
    pub(crate) fn on(trading_date: NaiveDate, positions: &'a [MarginPosition]) -> Self {
        Self {
            trading_date,
            positions,
            exposure: None,
            contracts: None,
            cash: None,
        }
    }

    pub(crate) fn exposure(mut self, exposure: &'a ExposureRates) -> Self {
        self.exposure = Some(exposure);
        self
    }

    pub(crate) fn contracts(mut self, contracts: &'a ContractElmRates) -> Self {
        self.contracts = Some(contracts);
        self
    }

    pub(crate) fn cash(mut self, cash: &'a CashMarginRates) -> Self {
        self.cash = Some(cash);
        self
    }

    pub(crate) fn price_xml(self, bytes: &[u8]) -> Result<SpanMarginQuote, String> {
        let trading_date = self.trading_date;
        self.finish(|specs| extract::extract_contracts(bytes, trading_date, specs))
    }

    pub(crate) fn price_index(self, index: &SpanFileIndex) -> Result<SpanMarginQuote, String> {
        let trading_date = self.trading_date;
        self.finish(|specs| index.book_for(trading_date, specs))
    }

    fn finish(
        self,
        load: impl FnOnce(&[SymbolSpec]) -> Result<Book, String>,
    ) -> Result<SpanMarginQuote, String> {
        let exposure = ExposureRates::none();
        let contracts = ContractElmRates::default();
        let cash = CashMarginRates::default();
        QuoteInputs {
            trading_date: self.trading_date,
            positions: self.positions,
            exposure: self.exposure.unwrap_or(&exposure),
            contracts: self.contracts.unwrap_or(&contracts),
            cash: self.cash.unwrap_or(&cash),
        }
        .price(load)
    }
}
