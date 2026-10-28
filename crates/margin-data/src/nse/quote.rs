//! NSCCL margin quote. The SPAN array is shared. Exposure, contract ELM,
//! the contract master, and cash VaR are the NSE files for the trade date.

use crate::DataError;
use crate::nse::contract_master::ContractMaster;
use crate::postgres::{ChosenSpan, PgStore, finish_span_quote};
use crate::span::quote::{
    CashMarginRates, ContractElmRates, ExposureRates, MarginPosition, quote_span_zip,
};
use margin_domain::{ClearingCorporation, Exchange};

impl PgStore {
    pub(crate) async fn quote_nse(
        &self,
        clearing: ClearingCorporation,
        selected: &ChosenSpan<'_>,
        positions: &[MarginPosition],
    ) -> Result<crate::postgres::StoredSpanQuote, DataError> {
        let trading_date = selected.trading_date;
        let exchange = Exchange::Nse;
        let exposure_bytes = self.exposure_file(exchange, clearing, trading_date).await?;
        let exposure = ExposureRates::parse_csv(&exposure_bytes, trading_date)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        let contract_bytes = self
            .contract_elm_file(exchange, clearing, trading_date)
            .await?;
        let contracts = ContractElmRates::parse_gzip(&contract_bytes)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        let master_bytes = self
            .contract_master_file(exchange, clearing, trading_date)
            .await?;
        let master = ContractMaster::parse_gzip(&master_bytes)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        let cash = match self.cash_var_file(exchange, clearing, trading_date).await? {
            Some(cash_bytes) => CashMarginRates::parse(&cash_bytes, trading_date)
                .map_err(|detail| DataError::MarginRejected { detail })?,
            None => CashMarginRates::default(),
        };
        let mut quote = quote_span_zip(
            selected.bytes,
            trading_date,
            positions,
            &exposure,
            &contracts,
            &cash,
        )
        .map_err(|detail| DataError::MarginRejected { detail })?;
        quote
            .apply_lot_sizes(&master)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        Ok(finish_span_quote(
            selected.chosen,
            trading_date,
            selected.trading_datetime,
            selected.span_created,
            quote,
        ))
    }
}
