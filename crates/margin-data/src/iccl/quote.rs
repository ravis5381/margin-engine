//! ICCL margin quote for a BSE SENSEX future. Extreme loss comes from the
//! ICCL file. NSE exposure percents are not applied.

use crate::DataError;
use crate::iccl::ingest::ICCL_ELM_KIND;
use crate::nse::contract_master::ContractMaster;
use crate::postgres::{
    ChosenSpan, PgStore, margin_rejected, one_auxiliary_hash, price_chosen_span,
};
use crate::span::quote::{ContractElmRates, MarginPosition, QuoteBuilder};
use chrono::NaiveDate;
use margin_domain::{ClearingCorporation, Exchange};

impl PgStore {
    pub(crate) async fn quote_iccl(
        &self,
        selected: &ChosenSpan<'_>,
        positions: &[MarginPosition],
    ) -> Result<crate::postgres::StoredSpanQuote, DataError> {
        let elm_bytes = self.iccl_elm_file(selected.trading_date).await?;
        let contracts = ContractElmRates::parse_iccl_zip(&elm_bytes).map_err(margin_rejected)?;
        price_chosen_span(
            selected,
            QuoteBuilder::on(selected.trading_date, positions).contracts(&contracts),
            &ContractMaster::default(),
        )
    }

    async fn iccl_elm_file(&self, trading_date: NaiveDate) -> Result<Vec<u8>, DataError> {
        let mut rows = self
            .auxiliary_rows(
                Exchange::Bse,
                ClearingCorporation::Iccl,
                trading_date,
                ICCL_ELM_KIND,
                "container_checked",
            )
            .await?;
        if rows.is_empty() {
            // The bytes are immutable. A parser rejection stays on the row, and the
            // quote parses those bytes again.
            rows = self
                .auxiliary_rows(
                    Exchange::Bse,
                    ClearingCorporation::Iccl,
                    trading_date,
                    ICCL_ELM_KIND,
                    "rejected",
                )
                .await?;
        }
        if rows.is_empty() {
            return Err(margin_rejected(format!(
                "no ICCL extreme loss file is archived for {trading_date}, so extreme loss margin is not calculated"
            )));
        }
        one_auxiliary_hash(
            rows,
            format!("more than one ICCL extreme loss file hash is archived for {trading_date}"),
        )
    }
}
