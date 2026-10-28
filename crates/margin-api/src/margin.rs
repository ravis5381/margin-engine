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
#[serde(rename_all = "camelCase")]
#[schema(
    description = "Combined margin for the book, in INR.",
    example = json!({
        "netPremium": "0",
        "SPANMargin": "6820.54",
        "exposureMargin": "1466.2",
        "marginBenefit": "0",
        "totalMargin": "8286.74"
    })
)]
pub struct CombinedMarginBody {
    /// Signed option premium. A short option is negative. A future is `0`.
    net_premium: String,
    /// SPAN margin: greatest scenario loss, plus the calendar-spread charge.
    #[serde(rename = "SPANMargin")]
    span_margin: String,
    /// Extreme loss margin. Brokers show this as exposure margin.
    exposure_margin: String,
    /// Amount saved by holding the positions together. `0` when there is no saving.
    margin_benefit: String,
    /// SPAN margin plus exposure margin plus delivery margin. A short option premium is included.
    total_margin: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(description = "One position in the order it was sent.")]
pub struct PositionMarginBody {
    /// `NSE` or `BSE`.
    exchange: String,
    /// Symbol that was matched to SPAN.
    contract: String,
    /// `FUTURE`, `OPTION`, or `EQUITY`.
    product: String,
    /// Strike in rupees. `0` for a future.
    strike_price: String,
    /// `FUT`, `CE`, `PE`, or `EQ`.
    instrument_type: String,
    /// `BUY` or `SELL`.
    trade_type: String,
    /// `CE` or `PE`. Empty for a future.
    option_type: String,
    /// Quantity in units. Always positive. The side is `tradeType`.
    qty: String,
    /// SPAN margin for this line before the book is netted.
    #[serde(rename = "SPANMargin")]
    span_margin: String,
    /// Extreme loss margin for this line.
    exposure_margin: String,
    /// Line total before portfolio netting. The book total can be lower.
    total_margin: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(
    description = "Margin quote. Amounts are decimal strings in INR. A calculated zero is `0`.",
    example = json!({
        "margin": {
            "netPremium": "0",
            "SPANMargin": "6820.54",
            "exposureMargin": "1466.2",
            "marginBenefit": "0",
            "totalMargin": "8286.74"
        },
        "positionMargin": [{
            "exchange": "BSE",
            "contract": "BSE:SENSEX:2026-10-29:FUT",
            "product": "FUTURE",
            "strikePrice": "0",
            "instrumentType": "FUT",
            "tradeType": "SELL",
            "optionType": "",
            "qty": "1",
            "SPANMargin": "6820.54",
            "exposureMargin": "1466.2",
            "totalMargin": "8286.74"
        }]
    })
)]
pub struct MarginResponse {
    /// Combined SPAN, exposure, premium, benefit, and total.
    margin: CombinedMarginBody,
    /// One row per position, in request order.
    position_margin: Vec<PositionMarginBody>,
}

#[utoipa::path(
    post,
    path = "/api/v1/margin/calculate",
    tag = "margin",
    operation_id = "calculateMargin",
    summary = "Quote margin for a book",
    description = "Returns the margin for one book.\n\n\
`margin.totalMargin` is SPAN margin plus exposure margin plus delivery margin. A short option premium is included. `margin.netPremium` is the signed option premium and is not added again. `margin.marginBenefit` is the saving from holding the positions together.\n\n\
The SPAN file is the latest snapshot published at or before `trading_datetime`. A missing file or contract is an error. A calculated zero is `0`. This quote does not replicate NSE Clearing or ICCL.",
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
                    margin: CombinedMarginBody {
                        net_premium: money(quote.premium),
                        span_margin: money(quote.span_margin),
                        exposure_margin: money(quote.extreme_loss_margin),
                        margin_benefit: money(quote.margin_benefit),
                        total_margin: money(quote.total_margin),
                    },
                    position_margin: quote
                        .lines
                        .into_iter()
                        .map(|line| position_margin(exchange, line))
                        .collect(),
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

fn position_margin(exchange: Exchange, line: margin_data::MarginLine) -> PositionMarginBody {
    let sell = line.quantity.is_sign_negative();
    let qty = if sell {
        Decimal::ZERO
            .checked_sub(line.quantity)
            .unwrap_or(line.quantity)
    } else {
        line.quantity
    };
    let (product, instrument_type, option_type) = match line.instrument {
        "call" => ("OPTION", "CE", "CE"),
        "put" => ("OPTION", "PE", "PE"),
        "stock" => ("EQUITY", "EQ", ""),
        _ => ("FUTURE", "FUT", ""),
    };
    PositionMarginBody {
        exchange: exchange.code().to_owned(),
        contract: line.symbol,
        product: product.to_owned(),
        strike_price: line.strike.map(money).unwrap_or_else(|| "0".to_owned()),
        instrument_type: instrument_type.to_owned(),
        trade_type: if sell { "SELL" } else { "BUY" }.to_owned(),
        option_type: option_type.to_owned(),
        qty: money(qty),
        span_margin: money(line.scanning_risk),
        exposure_margin: money(line.extreme_loss_margin),
        total_margin: money(line.total_margin),
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
    description = "Builds the symbol used by the margin call.\n\n\
A future is `NSE:NIFTY:2026-11-23:FUT`. An option adds the strike, as in `NSE:NIFTY:2026-11-03:26000:CE`. A stock is `NSE:RELIANCE:EQ`. `C` and `P` come back as `CE` and `PE`.\n\n\
This call does not read a file and does not return a margin. Send `canonical` as `symbol` on the margin call.",
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
