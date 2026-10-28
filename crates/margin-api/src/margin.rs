use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset, NaiveDateTime};
use margin_data::{
    AppStore, DataError, MarginPosition, canonical_contract_symbol, check_margin_positions,
    ensure_nse_margin_files,
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
    if exchange != Exchange::Nse {
        return Err("SPAN margin is available for archived NSE files".to_owned());
    }
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
    description = "One NSE trade datetime and its positions. The datetime selects the latest SPAN file whose created time is at or before that moment. A time without an offset is India Standard Time, the clock in the SPAN file. A position sends either a symbol or underlying, expiry, strike, and right. Sending both is rejected. This does not replicate NSE Clearing or ICCL."
)]
pub struct MarginRequest {
    #[schema(example = "NSE")]
    exchange: String,
    #[schema(example = "2026-10-28T14:30:00+05:30")]
    trading_datetime: String,
    positions: Vec<MarginPositionRequest>,
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "Either symbol, or underlying with right. Right is FUT, C, P, CE, PE, or EQ. A future needs expiry and no strike. An option needs expiry and a positive strike. Quantity is a non-zero decimal string."
)]
pub struct MarginPositionRequest {
    #[schema(example = "NSE:NIFTY:2026-11-23:FUT")]
    #[serde(default)]
    symbol: Option<String>,
    #[schema(example = "NIFTY")]
    #[serde(default)]
    underlying: Option<String>,
    #[schema(example = "2026-11-23")]
    #[serde(default)]
    expiry: Option<String>,
    #[schema(example = "26000")]
    #[serde(default)]
    strike: Option<String>,
    #[schema(example = "FUT")]
    #[serde(default)]
    right: Option<String>,
    #[schema(example = "-65")]
    quantity: String,
}

#[derive(Serialize, ToSchema)]
#[schema(
    description = "SPAN scanning risk, spread charge, extreme loss margin, option premium, delivery margin, and the combined total. Amounts are decimal strings in INR, rounded half away from zero to paise. The total is SPAN margin plus exposure margin plus delivery margin. A short option's premium is included. A book of only long options has a total equal to its delivery margin. Margin benefit is the reduction already reflected in a spread total, measured before delivery margin."
)]
pub struct MarginResponse {
    equivalence_claim: &'static str,
    method: &'static str,
    currency: &'static str,
    trading_date: String,
    trading_datetime: String,
    span_created: String,
    dataset_id: String,
    snapshot_class: String,
    intraday_sequence: Option<u32>,
    filename: String,
    sha256: String,
    scanning_risk: String,
    spread_charge: String,
    short_option_minimum: String,
    span_margin: String,
    extreme_loss_margin: String,
    premium: String,
    premium_margin: String,
    delivery_margin: String,
    cross_margin_benefit: String,
    margin_benefit: String,
    total_margin: String,
    positions: Vec<MarginLineBody>,
    note: &'static str,
}

#[derive(Serialize, ToSchema)]
pub struct MarginLineBody {
    symbol: String,
    canonical: String,
    instrument: String,
    product: String,
    expiry: Option<String>,
    strike: Option<String>,
    quantity: String,
    settlement_price: String,
    underlying_price: Option<String>,
    scanning_risk: String,
    extreme_loss_margin: String,
    elm_percent: String,
    premium: String,
    total_margin: String,
    lot_size: Option<u32>,
    worst_scan_points: Vec<u32>,
}

#[utoipa::path(
    post,
    path = "/api/v1/margin/calculate",
    tag = "margin",
    operation_id = "calculate_margin",
    summary = "SPAN, extreme loss margin, and premium",
    description = "Builds a canonical symbol from underlying, expiry, strike, and right, or accepts a symbol. The trade datetime selects the latest archived NSE SPAN file whose created time is at or before that moment, and the exposure, contract ELM, cash VaR, and contract master files for that date. An invalid book is rejected before those files are read. The total is SPAN margin plus exposure margin plus delivery margin. A short option premium is included. This does not replicate NSE Clearing or ICCL.",
    request_body = MarginRequest,
    responses(
        (status = 200, description = "SPAN scanning requirement", body = MarginResponse),
        (status = 404, description = "No SPAN file published at or before that datetime is archived", body = crate::ErrorBody),
        (status = 422, description = "The book or the as-of SPAN file cannot be valued, or a required file could not be downloaded", body = crate::ErrorBody),
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
    let stored = match state
        .store
        .span_margin(
            exchange,
            ClearingCorporation::Nscc,
            trading_datetime,
            &positions,
        )
        .await
    {
        Err(error) if missing_archive(&error) => {
            if let (Some(config), AppStore::Postgres(store)) = (&state.config, &state.store)
                && let Err(error) =
                    ensure_nse_margin_files(config, store, trading_datetime.date()).await
            {
                return download_error(error);
            }
            state
                .store
                .span_margin(
                    exchange,
                    ClearingCorporation::Nscc,
                    trading_datetime,
                    &positions,
                )
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
                    method: "nse-span-xml-4.00",
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
                || detail.contains("capital-market VaR rate is not in the archived files")
                || detail.contains("no contract master is archived")
        }
        _ => false,
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "Exchange, underlying, expiry, strike, and right. Right is FUT, C, P, CE, PE, or EQ. A future has no strike. An option needs a positive strike. EQ is a physical stock."
)]
pub struct CanonicalInstrumentRequest {
    #[schema(example = "NSE")]
    exchange: String,
    #[schema(example = "NIFTY")]
    underlying: String,
    #[schema(example = "2026-11-23")]
    #[serde(default)]
    expiry: Option<String>,
    #[schema(example = "26000")]
    #[serde(default)]
    strike: Option<String>,
    #[schema(example = "FUT")]
    right: String,
}

#[derive(Serialize, ToSchema)]
#[schema(
    description = "Canonical instrument. A future is NSE:UNDERLYING:YYYY-MM-DD:FUT. An option is NSE:UNDERLYING:YYYY-MM-DD:STRIKE:CE or PE. A stock is NSE:UNDERLYING:EQ."
)]
pub struct CanonicalInstrumentResponse {
    canonical: String,
    exchange: String,
    underlying: String,
    expiry: Option<String>,
    strike: Option<String>,
    right: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/instruments/canonical",
    tag = "instruments",
    operation_id = "canonical_instrument",
    summary = "Canonical instrument symbol",
    description = "Builds NSE:UNDERLYING:YYYY-MM-DD:FUT, NSE:UNDERLYING:YYYY-MM-DD:STRIKE:CE, NSE:UNDERLYING:YYYY-MM-DD:STRIKE:PE, or NSE:UNDERLYING:EQ from the exchange, underlying, expiry, strike, and right. It does not read an archived file and does not return a margin.",
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
