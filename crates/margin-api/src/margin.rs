use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::NaiveDate;
use margin_data::{DataError, MarginPosition};
use margin_domain::{ClearingCorporation, Exchange};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa::ToSchema;

use crate::{AppState, error_response};

fn money(amount: Decimal) -> String {
    amount.normalize().to_string()
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "Book to value from the archived SPAN file for one trading date. Symbols may be a monthly future (NIFTY26NOVFUT), a dated future (RELIANCE26N26FUT), a monthly or dated option (NIFTY26NOV26000CE, NIFTY26N0326000CE), or a physical stock (RELIANCE)."
)]
pub struct MarginRequest {
    #[schema(example = "NSE")]
    exchange: String,
    #[schema(example = "2026-10-28")]
    trading_date: String,
    positions: Vec<MarginPositionRequest>,
}

#[derive(Deserialize, ToSchema)]
pub struct MarginPositionRequest {
    #[schema(example = "NIFTY26NOVFUT")]
    symbol: String,
    #[schema(example = "-65")]
    quantity: String,
}

#[derive(Serialize, ToSchema)]
#[schema(
    description = "SPAN scanning risk, spread charge, extreme loss margin, option premium, delivery margin, and the combined total. Amounts are decimal strings in INR, rounded half away from zero to paise. The total is SPAN margin plus exposure margin. A short option's premium is included. A book of only long options has a total of zero. Margin benefit is the reduction already reflected in a spread total."
)]
pub struct MarginResponse {
    equivalence_claim: &'static str,
    method: &'static str,
    currency: &'static str,
    trading_date: String,
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
    worst_scan_points: Vec<u32>,
}

#[utoipa::path(
    post,
    path = "/api/v1/margin/calculate",
    tag = "margin",
    operation_id = "calculate_margin",
    summary = "SPAN, extreme loss margin, and premium",
    description = "Reads the newest official SPAN file, the exposure file, and the contract-wise ELM file archived for the trading date. A loss in the risk array is positive for a long unit. The greatest scenario loss is the SPAN scanning requirement. Offsetting expiries add the flat calendar-spread charge from the SPAN file. An index contract expiring on the trade date is not offset. Monthly symbols use YY and MON. Dated symbols use YY, a month code 1-9, O, N, or D, and DD. A physical stock is the product code. The short-option minimum is zero when the file's rate is zero. Extreme loss margin uses the contract file's percent. A futures calendar spread charges one third of that percent on the far-month quantity. Short-option notional is the SPAN underlying price. Option premium is the SPAN price times quantity. Payable premium is the positive net premium. Delivery margin is zero outside the four trading days before a stock expiry. The total is SPAN margin plus exposure margin, which is the combined total shown by a broker margin calculator. A short option's premium is included in that total. A book of only long options has a total of zero. Margin benefit is the amount by which a spread total is below the sum of the separate position totals, and it is already reflected in the total. Cross-margin benefit is zero. Reported amounts are rounded half away from zero to paise. This is not a reconciliation to NSE Clearing, ICCL, or a broker calculator.",
    request_body = MarginRequest,
    responses(
        (status = 200, description = "SPAN scanning requirement", body = MarginResponse),
        (status = 404, description = "No official SPAN file is archived for that date", body = crate::ErrorBody),
        (status = 422, description = "The book or the newest SPAN file cannot be valued", body = crate::ErrorBody),
        (status = 503, description = "The archive could not be read", body = crate::ErrorBody)
    )
)]
pub async fn calculate_margin(
    State(state): State<AppState>,
    Json(request): Json<MarginRequest>,
) -> Response {
    let exchange = match Exchange::parse(&request.exchange) {
        Ok(exchange) => exchange,
        Err(error) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                error.to_string(),
            );
        }
    };
    if exchange != Exchange::Nse {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "SPAN margin is available for archived NSE files".to_owned(),
        );
    }
    let trading_date = match NaiveDate::parse_from_str(&request.trading_date, "%Y-%m-%d") {
        Ok(date) => date,
        Err(_) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                "trading_date must be YYYY-MM-DD".to_owned(),
            );
        }
    };
    let mut positions = Vec::new();
    for position in request.positions {
        let quantity = match Decimal::from_str(position.quantity.trim()) {
            Ok(quantity) => quantity,
            Err(_) => {
                return error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_request",
                    format!("quantity for {} must be a decimal string", position.symbol),
                );
            }
        };
        positions.push(MarginPosition {
            symbol: position.symbol,
            quantity,
        });
    }
    match state
        .store
        .span_margin(
            exchange,
            ClearingCorporation::Nscc,
            trading_date,
            &positions,
        )
        .await
    {
        Ok(stored) => {
            let quote = stored.quote;
            (
                StatusCode::OK,
                Json(MarginResponse {
                    equivalence_claim: margin_domain::EQUIVALENCE_CLAIM,
                    method: "nse-span-xml-4.00",
                    currency: quote.currency,
                    trading_date: stored.trading_date.to_string(),
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
