//! Choose the SPAN file current at the trade time, then price that file.

use super::{PgStore, db_error_message};
use crate::DataError;
use crate::nse::contract_master::ContractMaster;
use crate::nse::reports::SPAN_FILE_FORMAT;
use crate::span::quote::{MarginPosition, QuoteBuilder, SpanFileIndex, SpanMarginQuote};
use crate::span::xml::span_created_timestamp;
use chrono::{NaiveDate, NaiveDateTime};
use margin_domain::{ClearingCorporation, Exchange};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct SpanCandidate {
    id: Uuid,
    snapshot_class: String,
    intraday_sequence: Option<i32>,
    original_filename: String,
    sha256: String,
    publication_status: String,
    validation_status: String,
    prefix: Vec<u8>,
}

pub(crate) struct ChosenSpan<'a> {
    pub chosen: &'a SpanCandidate,
    pub span_created: NaiveDateTime,
    pub trading_date: NaiveDate,
    pub trading_datetime: NaiveDateTime,
    pub index: Arc<SpanFileIndex>,
}

pub struct StoredSpanQuote {
    pub dataset_id: Uuid,
    pub trading_date: NaiveDate,
    pub trading_datetime: NaiveDateTime,
    pub span_created: NaiveDateTime,
    pub snapshot_class: String,
    pub intraday_sequence: Option<u32>,
    pub filename: String,
    pub sha256: String,
    pub quote: SpanMarginQuote,
}

impl PgStore {
    pub async fn span_margin(
        &self,
        exchange: Exchange,
        clearing: ClearingCorporation,
        trading_datetime: NaiveDateTime,
        positions: &[MarginPosition],
    ) -> Result<StoredSpanQuote, DataError> {
        let trading_date = trading_datetime.date();
        let candidates = sqlx::query_as::<_, SpanCandidate>(
            "SELECT d.id, d.snapshot_class, d.intraday_sequence, d.original_filename, d.sha256, \
             d.publication_status, d.validation_status, \
             substring(f.content from 1 for 80000) AS prefix \
             FROM risk_datasets d \
             JOIN risk_files f ON f.sha256 = d.sha256 \
             WHERE d.exchange = $1 AND d.clearing_corporation = $2 AND d.trading_date = $3 \
               AND d.origin = 'official' AND d.file_format = $4 \
               AND d.parser_version = 'span-xml-4.00'",
        )
        .bind(exchange.code())
        .bind(clearing.code())
        .bind(trading_date)
        .bind(span_file_format(exchange))
        .fetch_all(&self.pool)
        .await
        .map_err(db_error_message)?;
        let (chosen, span_created) = choose_span_as_of(&candidates, trading_datetime)?;
        let index = if let Some(index) = self.span_indexes.cached(&chosen.sha256) {
            index
        } else {
            let bytes: Vec<u8> =
                sqlx::query_scalar("SELECT content FROM risk_files WHERE sha256 = $1")
                    .bind(&chosen.sha256)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(db_error_message)?;
            self.span_indexes
                .get_or_build(&chosen.sha256, &bytes)
                .map_err(margin_rejected)?
        };
        let selected = ChosenSpan {
            chosen,
            span_created,
            trading_date,
            trading_datetime,
            index,
        };
        match exchange {
            Exchange::Nse => self.quote_nse(clearing, &selected, positions).await,
            Exchange::Bse => self.quote_iccl(&selected, positions).await,
        }
    }
}

pub(crate) fn margin_rejected(detail: String) -> DataError {
    DataError::MarginRejected { detail }
}

pub(crate) fn price_chosen_span(
    selected: &ChosenSpan<'_>,
    quote: QuoteBuilder<'_>,
    master: &ContractMaster,
) -> Result<StoredSpanQuote, DataError> {
    let mut quote = quote
        .price_index(&selected.index)
        .map_err(margin_rejected)?;
    quote.apply_lot_sizes(master).map_err(margin_rejected)?;
    Ok(finish_span_quote(
        selected.chosen,
        selected.trading_date,
        selected.trading_datetime,
        selected.span_created,
        quote,
    ))
}

pub(crate) fn finish_span_quote(
    chosen: &SpanCandidate,
    trading_date: NaiveDate,
    trading_datetime: NaiveDateTime,
    span_created: NaiveDateTime,
    quote: SpanMarginQuote,
) -> StoredSpanQuote {
    StoredSpanQuote {
        dataset_id: chosen.id,
        trading_date,
        trading_datetime,
        span_created,
        snapshot_class: chosen.snapshot_class.clone(),
        intraday_sequence: chosen
            .intraday_sequence
            .map(|sequence| u32::try_from(sequence).unwrap_or(u32::MAX)),
        filename: chosen.original_filename.clone(),
        sha256: chosen.sha256.clone(),
        quote,
    }
}

fn span_file_format(exchange: Exchange) -> &'static str {
    match exchange {
        Exchange::Nse => SPAN_FILE_FORMAT,
        Exchange::Bse => crate::iccl::ingest::ICCL_SPAN_FILE_FORMAT,
    }
}

fn choose_span_as_of(
    candidates: &[SpanCandidate],
    trading_datetime: NaiveDateTime,
) -> Result<(&SpanCandidate, NaiveDateTime), DataError> {
    let trading_date = trading_datetime.date();
    if candidates.is_empty() {
        return Err(DataError::MarginNotFound {
            detail: format!("no official SPAN file is archived for {trading_date}"),
        });
    }
    let mut eligible = Vec::new();
    for candidate in candidates {
        let created = span_created_timestamp(&candidate.prefix).map_err(|detail| {
            DataError::MarginRejected {
                detail: format!("{}: {detail}", candidate.original_filename),
            }
        })?;
        if created.date() != trading_date {
            return Err(DataError::MarginRejected {
                detail: format!(
                    "{} was created at {created}, which is not trading date {trading_date}",
                    candidate.original_filename
                ),
            });
        }
        if created <= trading_datetime {
            eligible.push((created, candidate));
        }
    }
    if eligible.is_empty() {
        return Err(DataError::MarginNotFound {
            detail: format!(
                "no SPAN file published at or before {trading_datetime} is archived for {trading_date}"
            ),
        });
    }
    let latest = eligible
        .iter()
        .map(|(created, _)| *created)
        .max()
        .expect("eligible");
    let top: Vec<&SpanCandidate> = eligible
        .into_iter()
        .filter(|(created, _)| *created == latest)
        .map(|(_, candidate)| candidate)
        .collect();
    let mut hashes: Vec<&str> = top
        .iter()
        .map(|candidate| candidate.sha256.as_str())
        .collect();
    hashes.sort_unstable();
    hashes.dedup();
    if hashes.len() != 1 {
        return Err(DataError::MarginRejected {
            detail: format!("the SPAN file created at {latest} has more than one file hash"),
        });
    }
    let chosen = top[0];
    if chosen.publication_status == "rejected" || chosen.validation_status == "failed" {
        return Err(DataError::MarginRejected {
            detail: format!(
                "the SPAN snapshot {} current at {latest} is {} and {}; an older snapshot was not substituted",
                chosen.original_filename, chosen.publication_status, chosen.validation_status
            ),
        });
    }
    if chosen.publication_status != "draft" && chosen.publication_status != "published" {
        return Err(DataError::MarginRejected {
            detail: format!(
                "the SPAN snapshot {} current at {latest} has publication status {}",
                chosen.original_filename, chosen.publication_status
            ),
        });
    }
    Ok((chosen, latest))
}

#[cfg(test)]
mod tests {
    use super::{SpanCandidate, choose_span_as_of};
    use crate::DataError;
    use uuid::Uuid;

    #[test]
    fn the_latest_span_file_at_or_before_the_trade_time_is_chosen() {
        let early = span_file(
            "nsccl.20261028.i1.zip",
            "aa",
            "begin_of_day",
            None,
            "202610280201",
        );
        let mid = span_file(
            "nsccl.20261028.i4.zip",
            "bb",
            "intraday",
            Some(3),
            "202610281403",
        );
        let late = span_file(
            "nsccl.20261028.s.zip",
            "cc",
            "end_of_day",
            None,
            "202610281703",
        );
        let files = [early, mid, late];
        let afternoon = stamp("2026-10-28T14:30:00");
        let chosen = choose_span_as_of(&files, afternoon).expect("afternoon");
        assert_eq!(chosen.0.original_filename, "nsccl.20261028.i4.zip");
        assert_eq!(chosen.1, stamp("2026-10-28T14:03:00"));
        let evening = choose_span_as_of(&files, stamp("2026-10-28T18:00:00")).expect("evening");
        assert_eq!(evening.0.original_filename, "nsccl.20261028.s.zip");
        let before_open = choose_span_as_of(&files, stamp("2026-10-28T01:00:00"));
        assert!(matches!(before_open, Err(DataError::MarginNotFound { .. })));
        let mut rejected = files;
        rejected[1].publication_status = "rejected".to_owned();
        let blocked = choose_span_as_of(&rejected, afternoon);
        assert!(matches!(blocked, Err(DataError::MarginRejected { .. })));
    }

    fn stamp(value: &str) -> chrono::NaiveDateTime {
        chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").expect("stamp")
    }

    fn span_file(
        name: &str,
        hash: &str,
        class: &str,
        sequence: Option<i32>,
        created: &str,
    ) -> SpanCandidate {
        let payload = format!("<spanFile><created>{created}</created></spanFile>");
        let bytes = payload.as_bytes();
        let member = b"member.spn";
        let mut prefix = Vec::new();
        prefix.extend_from_slice(b"PK\x03\x04");
        prefix.extend_from_slice(&20_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(&0_u32.to_le_bytes());
        prefix.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        prefix.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        prefix.extend_from_slice(&(member.len() as u16).to_le_bytes());
        prefix.extend_from_slice(&0_u16.to_le_bytes());
        prefix.extend_from_slice(member);
        prefix.extend_from_slice(bytes);
        SpanCandidate {
            id: Uuid::nil(),
            snapshot_class: class.to_owned(),
            intraday_sequence: sequence,
            original_filename: name.to_owned(),
            sha256: hash.to_owned(),
            publication_status: "draft".to_owned(),
            validation_status: "pending".to_owned(),
            prefix,
        }
    }
}
