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
    let right = filled(&position.instrument_type);
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
            right.ok_or_else(|| "instrumentType must be FUT, C, P, CE, PE, or EQ".to_owned())?,
        ),
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "One NSE or BSE book valued at a trade time. A position sends contract fields or a symbol.",
    examples(
        json!({
            "exchange": "NSE",
            "trading_datetime": "2026-10-28T14:30:00+05:30",
            "positions": [{
                "underlying": "NIFTY",
                "expiry": "2026-11-23",
                "instrumentType": "FUT",
                "quantity": "-65"
            }]
        }),
        json!({
            "exchange": "NSE",
            "trading_datetime": "2026-10-28T14:30:00+05:30",
            "positions": [{
                "symbol": "NSE:NIFTY:2026-11-23:FUT",
                "quantity": "-65"
            }]
        })
    )
)]
pub struct MarginRequest {
    /// `NSE` or `BSE`. `BSE` quotes SENSEX from the ICCL SPAN file.
    #[schema(example = "NSE")]
    exchange: String,
    /// Trade time. No offset means India Standard Time. `2026-10-28T14:30:00+05:30` and `2026-10-28T14:30:00` are the same moment.
    #[schema(example = "2026-10-28T14:30:00+05:30")]
    trading_datetime: String,
    /// The positions in the book. Symbols are matched case-insensitively and must be unique.
    positions: Vec<MarginPositionRequest>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(
    description = "One position. Send `symbol`, or send `underlying` with `instrumentType` and the fields that type requires. Sending both is rejected."
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
    instrument_type: Option<String>,
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
        "elm": "1466.2",
        "deliveryMargin": "0",
        "additionalMargin": "0",
        "tenderMargin": "0",
        "specialMargin": "0",
        "additionalPreExpiryMargin": "0",
        "marginBenefit": "0",
        "totalMargin": "8286.74"
    })
)]
pub struct CombinedMarginBody {
    /// Signed option premium. A short option is negative. A future is `0`.
    net_premium: String,
    /// Scanning risk plus the calendar-spread charge, adjusted by the net option premium. A short premium increases it. A long premium decreases it, and it is not negative.
    #[serde(rename = "SPANMargin")]
    span_margin: String,
    /// Extreme loss margin. Same amount as `elm`.
    exposure_margin: String,
    /// Extreme loss margin. Same amount as `exposureMargin`.
    elm: String,
    /// Delivery margin. `0` for an index and outside the stock delivery window.
    delivery_margin: String,
    /// Separate additional margin. An additional extreme-loss percent is already inside `elm`. This charge is `0`.
    additional_margin: String,
    /// Tender margin. This quote does not levy it. The amount is `0`.
    tender_margin: String,
    /// Special margin. This quote does not levy it. The amount is `0`.
    special_margin: String,
    /// Additional pre-expiry margin. This quote does not levy it. The amount is `0`.
    additional_pre_expiry_margin: String,
    /// Amount saved by holding the positions together. `0` when there is no saving.
    margin_benefit: String,
    /// SPAN margin plus extreme loss margin plus delivery margin. Premium is already inside SPAN margin.
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
    /// Signed option premium for this line. A short is negative. A future is `0`.
    premium: String,
    /// Scanning risk for this line adjusted by its premium, before the book is netted. A long option is `0` when the premium covers the scan.
    #[serde(rename = "SPANMargin")]
    span_margin: String,
    /// Extreme loss margin for this line. Same amount as `elm`.
    exposure_margin: String,
    /// Extreme loss margin for this line. Same amount as `exposureMargin`.
    elm: String,
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
            "elm": "1466.2",
            "deliveryMargin": "0",
            "additionalMargin": "0",
            "tenderMargin": "0",
            "specialMargin": "0",
            "additionalPreExpiryMargin": "0",
            "marginBenefit": "0",
            "totalMargin": "8286.74"
        },
        "totalPositionMargin": "8286.74",
        "positionMargin": [{
            "exchange": "BSE",
            "contract": "BSE:SENSEX:2026-10-29:FUT",
            "product": "FUTURE",
            "strikePrice": "0",
            "instrumentType": "FUT",
            "tradeType": "SELL",
            "optionType": "",
            "qty": "1",
            "premium": "0",
            "SPANMargin": "6820.54",
            "exposureMargin": "1466.2",
            "elm": "1466.2",
            "totalMargin": "8286.74"
        }]
    })
)]
pub struct MarginResponse {
    /// Combined SPAN, extreme loss, delivery, premium, benefit, and total.
    margin: CombinedMarginBody,
    /// Sum of the position totals before the book is netted.
    total_position_margin: String,
    /// One row per position, in request order.
    position_margin: Vec<PositionMarginBody>,
}

#[utoipa::path(
    post,
    path = "/api/v1/margin/calculate",
    tag = "margin",
    operation_id = "calculateMargin",
    summary = "Quote margin for a book",
    description = "Returns the margin for one book. Each position is either the contract fields or a `symbol`.\n\n\
`margin.SPANMargin` is the scanning risk plus the calendar-spread charge, plus a short option premium, minus a long option premium. It is `0` when the long premium covers the scan. `margin.netPremium` is that premium with its sign and is not added again. `margin.totalMargin` is SPAN margin plus extreme loss margin plus delivery margin. A long option book whose premium covers the scan has SPAN `0` and a total equal to its delivery margin. `margin.elm` is the extreme loss margin, the same amount as `margin.exposureMargin`. Additional, tender, special, and pre-expiry margin are `0`. `margin.marginBenefit` is the saving from holding the positions together. `totalPositionMargin` is the sum of the position totals before that saving.\n\n\
Each quote downloads SPAN snapshots for that trade date that are not already archived, including a snapshot published later the same day. The quote then uses the latest snapshot whose published time is at or before `trading_datetime`. A missing file or contract is an error. A calculated zero is `0`. This quote does not replicate NSE Clearing or ICCL.",
    request_body(
        content = MarginRequest,
        description = "Exchange, trade time, and the positions. A position sends contract fields or a symbol.",
        examples(
            ("Contract fields" = (
                summary = "Contract fields",
                description = "Underlying, expiry, and instrumentType.",
                value = json!({
                    "exchange": "NSE",
                    "trading_datetime": "2026-10-28T14:30:00+05:30",
                    "positions": [{
                        "underlying": "NIFTY",
                        "expiry": "2026-11-23",
                        "instrumentType": "FUT",
                        "quantity": "-65"
                    }]
                })
            )),
            ("Symbol" = (
                summary = "Symbol",
                description = "Canonical symbol in place of the contract fields.",
                value = json!({
                    "exchange": "NSE",
                    "trading_datetime": "2026-10-28T14:30:00+05:30",
                    "positions": [{
                        "symbol": "NSE:NIFTY:2026-11-23:FUT",
                        "quantity": "-65"
                    }]
                })
            ))
        )
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
    let refresh = match (&state.config, &state.store) {
        (Some(config), AppStore::Postgres(store)) => Some(match exchange {
            Exchange::Nse => ensure_nse_margin_files(config, store, trading_datetime.date()).await,
            Exchange::Bse => ensure_iccl_margin_files(config, store, trading_datetime.date()).await,
        }),
        _ => None,
    };
    let stored = match state
        .store
        .span_margin(exchange, clearing, trading_datetime, &positions)
        .await
    {
        Err(error) if missing_archive(&error) => match refresh {
            Some(Err(download)) => return download_error(download),
            _ => Err(error),
        },
        other => other,
    };
    match stored {
        Ok(stored) => match quote_body(exchange, stored.quote) {
            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
            Err(detail) => {
                error_response(StatusCode::UNPROCESSABLE_ENTITY, "margin_rejected", detail)
            }
        },
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

const NO_CHARGE: &str = "0";

fn quote_body(
    exchange: Exchange,
    quote: margin_data::SpanMarginQuote,
) -> Result<MarginResponse, String> {
    let total_position_margin = quote
        .lines
        .iter()
        .try_fold(Decimal::ZERO, |total, line| {
            total.checked_add(line.total_margin)
        })
        .ok_or_else(|| "position total overflowed".to_owned())?;
    let span_margin = span_after_premium(quote.span_margin, quote.premium)?;
    let total_margin = span_margin
        .checked_add(quote.extreme_loss_margin)
        .and_then(|amount| amount.checked_add(quote.delivery_margin))
        .ok_or_else(|| "total margin overflowed".to_owned())?;
    let before_delivery = total_margin
        .checked_sub(quote.delivery_margin)
        .ok_or_else(|| "total margin overflowed".to_owned())?;
    let margin_benefit = total_position_margin
        .checked_sub(before_delivery)
        .filter(|gap| gap.is_sign_positive())
        .unwrap_or(Decimal::ZERO);
    let elm = money(quote.extreme_loss_margin);
    let position_margin = quote
        .lines
        .into_iter()
        .map(|line| position_margin(exchange, line))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MarginResponse {
        margin: CombinedMarginBody {
            net_premium: money(quote.premium),
            span_margin: money(span_margin),
            exposure_margin: elm.clone(),
            elm,
            delivery_margin: money(quote.delivery_margin),
            additional_margin: NO_CHARGE.to_owned(),
            tender_margin: NO_CHARGE.to_owned(),
            special_margin: NO_CHARGE.to_owned(),
            additional_pre_expiry_margin: NO_CHARGE.to_owned(),
            margin_benefit: money(margin_benefit),
            total_margin: money(total_margin),
        },
        total_position_margin: money(total_position_margin),
        position_margin,
    })
}

/// Broker SPAN subtracts the signed option premium from the scanning risk.
/// A short premium is negative, so it increases SPAN. A long premium decreases it.
/// The result is zero when the long premium covers the scan.
fn span_after_premium(span: Decimal, premium: Decimal) -> Result<Decimal, String> {
    let net = span
        .checked_sub(premium)
        .ok_or_else(|| "SPAN margin overflowed".to_owned())?;
    Ok(if net.is_sign_negative() {
        Decimal::ZERO
    } else {
        net
    })
}

fn position_margin(
    exchange: Exchange,
    line: margin_data::MarginLine,
) -> Result<PositionMarginBody, String> {
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
    let elm = money(line.extreme_loss_margin);
    Ok(PositionMarginBody {
        exchange: exchange.code().to_owned(),
        contract: line.symbol,
        product: product.to_owned(),
        strike_price: line.strike.map(money).unwrap_or_else(|| "0".to_owned()),
        instrument_type: instrument_type.to_owned(),
        trade_type: if sell { "SELL" } else { "BUY" }.to_owned(),
        option_type: option_type.to_owned(),
        qty: money(qty),
        premium: money(line.premium),
        span_margin: money(span_after_premium(line.scanning_risk, line.premium)?),
        exposure_margin: elm.clone(),
        elm,
        total_margin: money(line.total_margin),
    })
}

#[cfg(test)]
mod tests {
    use super::span_after_premium;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn option_premium_adjusts_span_like_the_broker_calculator() {
        let span = Decimal::from_str("130887.6").expect("span");
        let premium = Decimal::from_str("-107568").expect("premium");
        assert_eq!(
            span_after_premium(span, premium).expect("span").to_string(),
            "238455.6"
        );
        let call = Decimal::from_str("106941.6").expect("call");
        let call_premium = Decimal::from_str("-17574").expect("call premium");
        assert_eq!(
            span_after_premium(call, call_premium)
                .expect("call span")
                .to_string(),
            "124515.6"
        );
        assert_eq!(
            span_after_premium(span, Decimal::ZERO).expect("future"),
            span
        );
        let long_scan = Decimal::from_str("35071.2").expect("long scan");
        let long_premium = Decimal::from_str("107568").expect("long premium");
        assert_eq!(
            span_after_premium(long_scan, long_premium).expect("long"),
            Decimal::ZERO
        );
        let covered = Decimal::from_str("17574").expect("covered");
        assert_eq!(
            span_after_premium(covered, covered).expect("covered"),
            Decimal::ZERO
        );
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
#[serde(rename_all = "camelCase")]
#[schema(
    description = "Contract identity. A future has an expiry and no strike. An option has an expiry and a positive strike. `EQ` is a physical stock and has neither.",
    example = json!({
        "exchange": "NSE",
        "underlying": "NIFTY",
        "expiry": "2026-11-23",
        "instrumentType": "FUT"
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
    instrument_type: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(
    description = "Canonical symbol and the fields used to build it. `C` and `P` are returned as `CE` and `PE`.",
    example = json!({
        "canonical": "NSE:NIFTY:2026-11-23:FUT",
        "exchange": "NSE",
        "underlying": "NIFTY",
        "expiry": "2026-11-23",
        "instrumentType": "FUT"
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
    instrument_type: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/instruments/canonical",
    tag = "instruments",
    operation_id = "canonicalInstrument",
    summary = "Build a canonical symbol",
    description = "Builds the canonical symbol from the exchange, product, expiry, strike, and instrumentType.\n\n\
A future is `NSE:NIFTY:2026-11-23:FUT`. An option adds the strike, as in `NSE:NIFTY:2026-11-03:26000:CE`. A stock is `NSE:RELIANCE:EQ`. `C` and `P` are returned as `CE` and `PE`.\n\n\
Pass the returned `canonical` value as `symbol` on the margin call.",
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
        &request.instrument_type,
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
        instrument_type: right.to_owned(),
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
