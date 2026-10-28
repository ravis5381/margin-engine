use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset, NaiveDateTime};
use margin_data::{
    AppStore, DataError, MarginPosition, canonical_contract_symbol, check_margin_positions,
    ensure_iccl_margin_files, ensure_nse_margin_files,
};
use margin_domain::{ClearingCorporation, Exchange};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa::ToSchema;

use crate::{AppState, error_response};

fn money(amount: Decimal) -> String {
    amount.normalize().to_string()
}

fn ist_stamp(value: NaiveDateTime) -> String {
    format!("{}+05:30", value.format("%Y-%m-%dT%H:%M:%S"))
}

fn parse_trading_datetime(value: &str) -> Result<NaiveDateTime, String> {
    let value = value.trim();
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST");
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&ist).naive_local());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(parsed);
        }
    }
    Err(
        "trading_datetime must be YYYY-MM-DDTHH:MM:SS in IST, or an RFC3339 datetime with an offset"
            .to_owned(),
    )
}

fn filled(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn prepare_book(
    request: &MarginRequest,
) -> Result<(Exchange, NaiveDateTime, Vec<MarginPosition>), String> {
    let exchange = Exchange::parse(request.exchange.trim()).map_err(|error| error.to_string())?;
    let trading_datetime = parse_trading_datetime(&request.trading_datetime)?;
    if request.positions.is_empty() {
        return Err("at least one position is required".to_owned());
    }
    if request.positions.len() > 32 {
        return Err("a margin quote accepts at most 32 positions".to_owned());
    }
    let mut positions = Vec::with_capacity(request.positions.len());
    for position in &request.positions {
        let symbol = position_symbol(exchange, position)?;
        let quantity = Decimal::from_str(position.quantity.trim())
            .map_err(|_| format!("quantity for {symbol} must be a decimal string"))?;
        if quantity == Decimal::ZERO {
            return Err(format!("quantity for {symbol} is zero"));
        }
        positions.push(MarginPosition { symbol, quantity });
    }
    check_margin_positions(&positions)?;
    Ok((exchange, trading_datetime, positions))
}

fn position_symbol(exchange: Exchange, position: &MarginPositionRequest) -> Result<String, String> {
    let underlying = filled(&position.underlying);
    let expiry = filled(&position.expiry);
    let strike = filled(&position.strike);
    let right = filled(&position.right);
    let structured =
        underlying.is_some() || expiry.is_some() || strike.is_some() || right.is_some();
    let symbol = filled(&position.symbol);
    match (symbol, structured) {
        (None, false) => {
            Err("a position needs a symbol or underlying, expiry, and FUT, C, or P".to_owned())
        }
        (Some(_), true) => Err("send a symbol or the contract fields, not both".to_owned()),
        (Some(symbol), false) => Ok(symbol.to_owned()),
        (None, true) => canonical_contract_symbol(
            exchange.code(),
            underlying.ok_or_else(|| "underlying must be the product code".to_owned())?,
            expiry,
            strike,
            right.ok_or_else(|| "right must be FUT, C, P, CE, PE, or EQ".to_owned())?,
        ),
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "One NSE or BSE book valued at a trade time. The example is a short 65-unit NIFTY November future. A BSE book uses underlying SENSEX.",
    example = json!({
        "exchange": "NSE",
        "trading_datetime": "2026-10-28T14:30:00+05:30",
        "positions": [{
            "underlying": "NIFTY",
            "expiry": "2026-11-23",
            "right": "FUT",
            "quantity": "-65"
        }]
    })
)]
pub struct MarginRequest {
    /// `NSE` or `BSE`. `BSE` quotes SENSEX from the ICCL SPAN file.
    #[schema(example = "NSE")]
    exchange: String,
    /// Trade time. No offset means India Standard Time. `2026-10-28T14:30:00+05:30` and `2026-10-28T14:30:00` are the same moment.
    #[schema(example = "2026-10-28T14:30:00+05:30")]
    trading_datetime: String,
    /// One to 32 positions. Symbols are matched case-insensitively and must be unique.
    positions: Vec<MarginPositionRequest>,
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "One position. Send `symbol`, or send `underlying` with `right` and the fields that right requires. Sending both is rejected."
)]
pub struct MarginPositionRequest {
    /// Market symbol or canonical symbol, such as `NIFTY26NOVFUT` or `NSE:NIFTY:2026-11-23:FUT`. Omit when the contract fields are set.
    #[schema(example = "NSE:NIFTY:2026-11-23:FUT")]
    #[serde(default)]
    symbol: Option<String>,
    /// Product code, such as `NIFTY`, `RELIANCE`, or `SENSEX`.
    #[schema(example = "NIFTY")]
    #[serde(default)]
    underlying: Option<String>,
    /// Contract expiry as `YYYY-MM-DD`. Required for a future or an option. Omit for `EQ`.
    #[schema(example = "2026-11-23")]
    #[serde(default)]
    expiry: Option<String>,
    /// Option strike in rupees, such as `26000`. Required and positive for an option. Omit for a future and for `EQ`.
    #[schema(example = "26000")]
    #[serde(default)]
    strike: Option<String>,
    /// `FUT`, `C`, `P`, `CE`, `PE`, or `EQ`. `C` and `P` are stored as `CE` and `PE`.
    #[schema(example = "FUT")]
    #[serde(default)]
    right: Option<String>,
    /// Signed quantity in units. `-65` is short 65 units. Zero is rejected.
    #[schema(example = "-65")]
    quantity: String,
}

#[derive(Serialize, ToSchema)]
#[schema(
    description = "Margin quote in INR. `total_margin` is SPAN margin plus extreme loss margin plus delivery margin. A short option premium is included in that total. Payable premium is reported separately."
)]
pub struct MarginResponse {
    /// `none` until this quote is reconciled to NSE Clearing or ICCL for the same book and file.
    equivalence_claim: &'static str,
    /// Parser identity. `nse-span-xml-4.00` for the current quote.
    method: &'static str,
    /// Always `INR`.
    currency: &'static str,
    /// Business date of the selected SPAN file, `YYYY-MM-DD`.
    trading_date: String,
    /// Requested trade time, normalized to India Standard Time.
    trading_datetime: String,
    /// `<created>` time of the SPAN file used for this quote, in India Standard Time.
    span_created: String,
    /// UUID of the archived SPAN dataset.
    dataset_id: String,
    /// `begin_of_day`, `intraday`, or `end_of_day`.
    snapshot_class: String,
    /// Intraday sequence `1` through `4`. Absent for begin-of-day and end-of-day.
    intraday_sequence: Option<u32>,
    /// Archived SPAN filename, such as `nsccl.20261028.i4.zip`.
    filename: String,
    /// SHA-256 of the archived SPAN zip.
    sha256: String,
    /// Greatest scenario loss across the book. Zero when every scenario is a gain.
    scanning_risk: String,
    /// Calendar-spread charge from the SPAN file, added to scanning risk.
    spread_charge: String,
    /// Short-option minimum. Zero when the SPAN file publishes a single zero rate.
    short_option_minimum: String,
    /// `scanning_risk` + `spread_charge` + `short_option_minimum`.
    span_margin: String,
    /// Extreme loss margin from the contract-wise ELM file.
    extreme_loss_margin: String,
    /// Signed option premium, `quantity * SPAN option price`. A short option is negative.
    premium: String,
    /// Payable premium, `max(0, premium)`. Reported beside the total and not added again.
    premium_margin: String,
    /// Delivery margin on stock derivatives inside the expiry window. Zero for an index and outside that window.
    delivery_margin: String,
    /// Always `0.00`. Index-constituent cross margin is not applied.
    cross_margin_benefit: String,
    /// Reduction already present in a spread, measured on position totals before delivery margin.
    margin_benefit: String,
    /// SPAN margin plus extreme loss margin plus delivery margin. A short option premium is included.
    total_margin: String,
    /// One line per accepted position, in request order.
    positions: Vec<MarginLineBody>,
    /// Which charges this book used.
    note: &'static str,
}

#[derive(Serialize, ToSchema)]
#[schema(description = "One position after it has been matched to a SPAN contract.")]
pub struct MarginLineBody {
    /// Symbol as submitted, or the canonical symbol built from the contract fields.
    symbol: String,
    /// `NSE:PRODUCT:YYYY-MM-DD:FUT`, `NSE:PRODUCT:YYYY-MM-DD:STRIKE:CE|PE`, or `NSE:PRODUCT:EQ`.
    canonical: String,
    /// `future`, `call`, `put`, or `stock`.
    instrument: String,
    /// SPAN product code.
    product: String,
    /// Contract expiry. Absent for a physical stock.
    expiry: Option<String>,
    /// Option strike in rupees. Absent for a future and for a stock.
    strike: Option<String>,
    /// Signed quantity in units.
    quantity: String,
    /// SPAN settlement price for this contract.
    settlement_price: String,
    /// SPAN underlying price. Present for an option.
    underlying_price: Option<String>,
    /// Greatest scenario loss for this line.
    scanning_risk: String,
    /// Extreme loss margin for this line.
    extreme_loss_margin: String,
    /// Contract ELM percent applied to this line. A spread can charge a smaller percent on the matched far month.
    elm_percent: String,
    /// Signed premium for this option. Zero for a future or a stock.
    premium: String,
    /// Line total before portfolio netting. The book total can be lower when a spread applies.
    total_margin: String,
    /// Lot size from the NSE F&O contract master. Absent for a physical stock. Not applied to quantity.
    lot_size: Option<u32>,
    /// SPAN scan-point numbers that produced the greatest loss.
    worst_scan_points: Vec<u32>,
}

#[utoipa::path(
    post,
    path = "/api/v1/margin/calculate",
    tag = "margin",
    operation_id = "calculateMargin",
    summary = "Quote margin for a book",
    description = "Values one NSE or BSE book on the latest archived SPAN snapshot whose created time is at or before `trading_datetime`.\n\n\
Exposure, contract ELM, cash-market VaR, and the F&O contract master are the NSE files for that calendar date. A BSE SENSEX book uses the ICCL SPAN file and the ICCL extreme-loss percent for that future month. SENSEX is the combined commodity named BSE 30 SENSEX. If a required file is missing, it is downloaded and the quote is retried once. A time before the first snapshot of the day returns 404.\n\n\
**Total** = SPAN margin + extreme loss margin + delivery margin. SPAN margin is the greatest scenario loss, plus the calendar-spread charge, plus a short-option minimum of zero when the file rate is zero. A futures spread charges one third of the far-month ELM percent on the matched quantity. A short option premium is included in the total. A book of only long options totals its delivery margin.\n\n\
An empty book, a zero quantity, a repeated symbol, a non-positive option strike, and a position that sends both a symbol and contract fields are rejected before the archive is read. This quote does not replicate NSE Clearing or ICCL.",
    request_body(
        content = MarginRequest,
        description = "Exchange, trade time, and one to 32 positions.",
        example = json!({
            "exchange": "NSE",
            "trading_datetime": "2026-10-28T14:30:00+05:30",
            "positions": [{
                "underlying": "NIFTY",
                "expiry": "2026-11-23",
                "right": "FUT",
                "quantity": "-65"
            }]
        })
    ),
    responses(
        (status = 200, description = "Margin quote for the SPAN file current at the trade time", body = MarginResponse),
        (status = 404, description = "No SPAN file was published at or before the trade time", body = crate::ErrorBody),
        (status = 408, description = "The quote exceeded the configured request timeout"),
        (status = 422, description = "The book is invalid, the SPAN file cannot value it, or a required file could not be downloaded", body = crate::ErrorBody),
        (status = 503, description = "The archive could not be read", body = crate::ErrorBody)
    )
)]
pub async fn calculate_margin(
    State(state): State<AppState>,
    Json(request): Json<MarginRequest>,
) -> Response {
    let (exchange, trading_datetime, positions) = match prepare_book(&request) {
        Ok(book) => book,
        Err(detail) => {
            return error_response(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", detail);
        }
    };
    let clearing = match exchange {
        Exchange::Nse => ClearingCorporation::Nscc,
        Exchange::Bse => ClearingCorporation::Iccl,
    };
    let stored = match state
        .store
        .span_margin(exchange, clearing, trading_datetime, &positions)
        .await
    {
        Err(error) if missing_archive(&error) => {
            if let (Some(config), AppStore::Postgres(store)) = (&state.config, &state.store)
                && let Err(error) = match exchange {
                    Exchange::Nse => {
                        ensure_nse_margin_files(config, store, trading_datetime.date()).await
                    }
                    Exchange::Bse => {
                        ensure_iccl_margin_files(config, store, trading_datetime.date()).await
                    }
                }
            {
                return download_error(error);
            }
            state
                .store
                .span_margin(exchange, clearing, trading_datetime, &positions)
                .await
        }
        other => other,
    };
    match stored {
        Ok(stored) => {
            let quote = stored.quote;
            (
                StatusCode::OK,
                Json(MarginResponse {
                    equivalence_claim: margin_domain::EQUIVALENCE_CLAIM,
                    method: match exchange {
                        Exchange::Nse => "nse-span-xml-4.00",
                        Exchange::Bse => "iccl-span-xml-4.00",
                    },
                    currency: quote.currency,
                    trading_date: stored.trading_date.to_string(),
                    trading_datetime: ist_stamp(stored.trading_datetime),
                    span_created: ist_stamp(stored.span_created),
                    dataset_id: stored.dataset_id.to_string(),
                    snapshot_class: stored.snapshot_class,
                    intraday_sequence: stored.intraday_sequence,
                    filename: stored.filename,
                    sha256: stored.sha256,
                    scanning_risk: quote.scanning_risk.to_string(),
                    spread_charge: quote.spread_charge.to_string(),
                    short_option_minimum: quote.short_option_minimum.to_string(),
                    span_margin: quote.span_margin.to_string(),
                    extreme_loss_margin: money(quote.extreme_loss_margin),
                    premium: quote.premium.to_string(),
                    premium_margin: quote.premium_margin.to_string(),
                    delivery_margin: quote.delivery_margin.to_string(),
                    cross_margin_benefit: quote.cross_margin_benefit.to_string(),
                    margin_benefit: quote.margin_benefit.to_string(),
                    total_margin: money(quote.total_margin),
                    positions: quote
                        .lines
                        .into_iter()
                        .map(|line| MarginLineBody {
                            symbol: line.symbol,
                            canonical: line.canonical,
                            instrument: line.instrument.to_owned(),
                            product: line.product,
                            expiry: line.expiry.map(|expiry| expiry.to_string()),
                            strike: line.strike.map(|strike| strike.to_string()),
                            quantity: line.quantity.to_string(),
                            settlement_price: line.settlement_price.to_string(),
                            underlying_price: line.underlying_price.map(|price| price.to_string()),
                            scanning_risk: line.scanning_risk.to_string(),
                            extreme_loss_margin: money(line.extreme_loss_margin),
                            elm_percent: money(line.elm_percent),
                            premium: line.premium.to_string(),
                            total_margin: money(line.total_margin),
                            lot_size: line.lot_size,
                            worst_scan_points: line.worst_scan_points,
                        })
                        .collect(),
                    note: quote.note,
                }),
            )
                .into_response()
        }
        Err(DataError::MarginNotFound { detail }) => {
            error_response(StatusCode::NOT_FOUND, "span_file_not_found", detail)
        }
        Err(DataError::MarginRejected { detail }) => {
            error_response(StatusCode::UNPROCESSABLE_ENTITY, "margin_rejected", detail)
        }
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "database_unavailable",
            error.to_string(),
        ),
    }
}

fn missing_archive(error: &DataError) -> bool {
    match error {
        DataError::MarginNotFound { .. } => true,
        DataError::MarginRejected { detail } => {
            detail.contains("no exposure file is archived")
                || detail.contains("no contract ELM file is archived")
                || detail.contains("no ICCL extreme loss file is archived")
                || detail.contains("capital-market VaR rate is not in the archived files")
                || detail.contains("no contract master is archived")
        }
        _ => false,
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "Contract identity. A future has an expiry and no strike. An option has an expiry and a positive strike. `EQ` is a physical stock and has neither.",
    example = json!({
        "exchange": "NSE",
        "underlying": "NIFTY",
        "expiry": "2026-11-23",
        "right": "FUT"
    })
)]
pub struct CanonicalInstrumentRequest {
    /// `NSE` or `BSE`.
    #[schema(example = "NSE")]
    exchange: String,
    /// Product code, such as `NIFTY`, `RELIANCE`, or `SENSEX`.
    #[schema(example = "NIFTY")]
    underlying: String,
    /// `YYYY-MM-DD`. Required for `FUT`, `C`, `P`, `CE`, and `PE`.
    #[schema(example = "2026-11-23")]
    #[serde(default)]
    expiry: Option<String>,
    /// Strike in rupees. Required for an option.
    #[schema(example = "26000")]
    #[serde(default)]
    strike: Option<String>,
    /// `FUT`, `C`, `P`, `CE`, `PE`, or `EQ`.
    #[schema(example = "FUT")]
    right: String,
}

#[derive(Serialize, ToSchema)]
#[schema(
    description = "Canonical symbol and the fields used to build it. `C` and `P` are returned as `CE` and `PE`.",
    example = json!({
        "canonical": "NSE:NIFTY:2026-11-23:FUT",
        "exchange": "NSE",
        "underlying": "NIFTY",
        "expiry": "2026-11-23",
        "right": "FUT"
    })
)]
pub struct CanonicalInstrumentResponse {
    /// `NSE:NIFTY:2026-11-23:FUT`, `NSE:NIFTY:2026-11-03:26000:CE`, or `NSE:RELIANCE:EQ`.
    canonical: String,
    /// Exchange code from the symbol.
    exchange: String,
    /// Product code from the symbol.
    underlying: String,
    /// Expiry. Absent for `EQ`.
    expiry: Option<String>,
    /// Strike in rupees. Present for an option.
    strike: Option<String>,
    /// `FUT`, `CE`, `PE`, or `EQ`.
    right: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/instruments/canonical",
    tag = "instruments",
    operation_id = "canonicalInstrument",
    summary = "Build a canonical symbol",
    description = "Builds the symbol the margin route matches to SPAN.\n\n\
| Right | Symbol |\n\
| --- | --- |\n\
| `FUT` | `NSE:UNDERLYING:YYYY-MM-DD:FUT` |\n\
| `C` or `CE` | `NSE:UNDERLYING:YYYY-MM-DD:STRIKE:CE` |\n\
| `P` or `PE` | `NSE:UNDERLYING:YYYY-MM-DD:STRIKE:PE` |\n\
| `EQ` | `NSE:UNDERLYING:EQ` |\n\n\
The route does not read an archived file and does not return a margin. Use the returned `canonical` as `symbol` on `POST /api/v1/margin/calculate`.",
    request_body = CanonicalInstrumentRequest,
    responses(
        (status = 200, description = "Canonical instrument", body = CanonicalInstrumentResponse),
        (status = 422, description = "The instrument fields cannot form a canonical symbol", body = crate::ErrorBody)
    )
)]
pub async fn canonical_instrument(Json(request): Json<CanonicalInstrumentRequest>) -> Response {
    let exchange = match Exchange::parse(request.exchange.trim()) {
        Ok(exchange) => exchange,
        Err(error) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                error.to_string(),
            );
        }
    };
    let canonical = match canonical_contract_symbol(
        exchange.code(),
        &request.underlying,
        filled(&request.expiry),
        filled(&request.strike),
        &request.right,
    ) {
        Ok(canonical) => canonical,
        Err(detail) => {
            return error_response(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", detail);
        }
    };
    Json(instrument_response(canonical)).into_response()
}

fn instrument_response(canonical: String) -> CanonicalInstrumentResponse {
    let parts: Vec<&str> = canonical.split(':').collect();
    let (expiry, strike, right) = match parts.as_slice() {
        [_, _, "EQ"] => (None, None, "EQ"),
        [_, _, expiry, "FUT"] => (Some((*expiry).to_owned()), None, "FUT"),
        [_, _, expiry, strike, right] => (
            Some((*expiry).to_owned()),
            Some((*strike).to_owned()),
            *right,
        ),
        _ => (None, None, ""),
    };
    CanonicalInstrumentResponse {
        exchange: parts.first().unwrap_or(&"").to_string(),
        underlying: parts.get(1).unwrap_or(&"").to_string(),
        expiry,
        strike,
        right: right.to_owned(),
        canonical,
    }
}

fn download_error(error: DataError) -> Response {
    match error {
        DataError::MarginNotFound { detail } => {
            error_response(StatusCode::NOT_FOUND, "span_file_not_found", detail)
        }
        DataError::MarginRejected { detail } => {
            error_response(StatusCode::UNPROCESSABLE_ENTITY, "margin_rejected", detail)
        }
        DataError::Database { detail } => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "database_unavailable",
            detail,
        ),
        other => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "download_failed",
            other.to_string(),
        ),
    }
}
