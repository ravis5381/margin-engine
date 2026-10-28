//! ICCL margin quote for a BSE SENSEX future. Extreme loss comes from the
//! ICCL file. NSE exposure percents are not applied.

use crate::DataError;
use crate::iccl::ingest::ICCL_ELM_KIND;
use crate::nse::contract_master::ContractMaster;
use crate::postgres::{ChosenSpan, PgStore, finish_span_quote, one_auxiliary_hash};
use crate::span::quote::{
    CashMarginRates, ContractElmRates, ExposureRates, MarginPosition, quote_span_zip,
};
use chrono::NaiveDate;
use margin_domain::{ClearingCorporation, Exchange};

impl PgStore {
    pub(crate) async fn quote_iccl(
        &self,
        selected: &ChosenSpan<'_>,
        positions: &[MarginPosition],
    ) -> Result<crate::postgres::StoredSpanQuote, DataError> {
        let elm_bytes = self.iccl_elm_file(selected.trading_date).await?;
        let contracts = ContractElmRates::parse_iccl_zip(&elm_bytes)
            .map_err(|detail| DataError::MarginRejected { detail })?;
        let mut quote = quote_span_zip(
            selected.bytes,
            selected.trading_date,
            positions,
            &ExposureRates::none(),
            &contracts,
            &CashMarginRates::default(),
        )
        .map_err(|detail| DataError::MarginRejected { detail })?;
        quote
            .apply_lot_sizes(&ContractMaster::default())
            .map_err(|detail| DataError::MarginRejected { detail })?;
        Ok(finish_span_quote(
            selected.chosen,
            selected.trading_date,
            selected.trading_datetime,
            selected.span_created,
            quote,
        ))
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
            return Err(DataError::MarginRejected {
                detail: format!(
                    "no ICCL extreme loss file is archived for {trading_date}, so extreme loss margin is not calculated"
                ),
            });
        }
        one_auxiliary_hash(
            rows,
            format!("more than one ICCL extreme loss file hash is archived for {trading_date}"),
        )
    }
}
