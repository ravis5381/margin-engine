//! NSCCL margin quote. The SPAN array is shared. Exposure, contract ELM,
//! and cash VaR are the NSE files for the trade date. The contract master is
//! that date's file, or the latest earlier archived file when NSE has not
//! published one yet.

use crate::DataError;
use crate::nse::contract_master::ContractMaster;
use crate::postgres::{ChosenSpan, PgStore, margin_rejected, price_chosen_span};
use crate::span::quote::{
    CashMarginRates, ContractElmRates, ExposureRates, MarginPosition, QuoteBuilder,
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
        let exposure =
            ExposureRates::parse_csv(&exposure_bytes, trading_date).map_err(margin_rejected)?;
        let contract_bytes = self
            .contract_elm_file(exchange, clearing, trading_date)
            .await?;
        let contracts = ContractElmRates::parse_gzip(&contract_bytes).map_err(margin_rejected)?;
        let master_bytes = self
            .contract_master_file(exchange, clearing, trading_date)
            .await?;
        let master = ContractMaster::parse_gzip(&master_bytes).map_err(margin_rejected)?;
        let cash = match self.cash_var_file(exchange, clearing, trading_date).await? {
            Some(cash_bytes) => {
                CashMarginRates::parse(&cash_bytes, trading_date).map_err(margin_rejected)?
            }
            None => CashMarginRates::default(),
        };
        price_chosen_span(
            selected,
            QuoteBuilder::on(trading_date, positions)
                .exposure(&exposure)
                .contracts(&contracts)
                .cash(&cash),
            &master,
        )
    }
}
