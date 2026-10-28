//! HTTP API. The margin route returns SPAN scanning risk, the calendar-spread
//! charge, extreme loss margin, option premium, and delivery margin. Cross-margin
//! benefit is zero. It does not
//! return a zero or null amount when the file or the contract is missing. A
//! calculated scanning risk of zero is returned when every scenario is a gain.
//!
//! Swagger UI at `/swagger-ui` documents only the routes registered here.

mod margin;
mod openapi;
mod readiness;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use margin_data::{AppConfig, AppStore, DatasetReadModel};
use margin_domain::{
    DatasetId, EQUIVALENCE_CLAIM, EQUIVALENCE_STATEMENT, OFFICIAL_CALCULATION_IMPLEMENTED,
};
use readiness::{ReadinessProbe, evaluate_readiness};
use serde::Serialize;
use std::time::Duration;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use utoipa::ToSchema;
use uuid::Uuid;

pub use readiness::{ReadinessCheck, ReadinessReport};

#[derive(Clone)]
pub struct AppState {
    pub store: AppStore,
    pub config: Option<AppConfig>,
    pub body_limit: usize,
    pub request_timeout: Duration,
    pub max_concurrency: usize,
}

pub fn router(state: AppState) -> Router {
    let timeout = state.request_timeout;
    let body_limit = state.body_limit;
    let concurrency = state.max_concurrency;
    Router::new()
        .merge(openapi::router())
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api/v1/data/status", get(data_status))
        .route("/api/v1/risk-data/datasets", get(list_datasets))
        .route("/api/v1/risk-data/datasets/{dataset_id}", get(get_dataset))
        .route("/api/v1/margin/calculate", post(margin::calculate_margin))
        .route(
            "/api/v1/instruments/canonical",
            post(margin::canonical_instrument),
        )
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
        .layer(ConcurrencyLimitLayer::new(concurrency))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                tracing::info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                )
            }),
        )
        .with_state(state)
}

#[derive(Serialize)]
struct HealthBody {
    /// Always `ok` when the process answered.
    status: &'static str,
    /// API version, the crate version of this build.
    version: &'static str,
    /// `none` until an official calculation is reconciled.
    equivalence_claim: &'static str,
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        equivalence_claim: EQUIVALENCE_CLAIM,
    })
}

#[derive(Serialize)]
struct ReadinessBody {
    /// `ready` or `not_ready`.
    status: &'static str,
    /// `none` until an official calculation is reconciled.
    equivalence_claim: &'static str,
    /// `database`, `official_validated_dataset`, and `official_calculation`, in that order.
    checks: Vec<ReadinessCheckBody>,
}

#[derive(Serialize)]
struct ReadinessCheckBody {
    /// `database`, `official_validated_dataset`, or `official_calculation`.
    name: &'static str,
    /// Whether this check passed.
    passed: bool,
    /// Why the check passed or failed.
    detail: String,
}

async fn ready(State(state): State<AppState>) -> Response {
    let (database_ok, database_detail) = match state.store.ping().await {
        Ok(()) => (true, "database connection succeeded".to_owned()),
        Err(error) => (false, error.to_string()),
    };
    let official_validated_datasets = if database_ok {
        match state.store.validated_dataset_count().await {
            Ok(count) => Some(count),
            Err(error) => {
                return readiness_response(evaluate_readiness(ReadinessProbe {
                    database_ok: false,
                    database_detail: error.to_string(),
                    official_validated_datasets: None,
                    official_calculation_implemented: OFFICIAL_CALCULATION_IMPLEMENTED,
                }));
            }
        }
    } else {
        None
    };
    readiness_response(evaluate_readiness(ReadinessProbe {
        database_ok,
        database_detail,
        official_validated_datasets,
        official_calculation_implemented: OFFICIAL_CALCULATION_IMPLEMENTED,
    }))
}

fn readiness_response(report: ReadinessReport) -> Response {
    let body = ReadinessBody {
        status: if report.ready { "ready" } else { "not_ready" },
        equivalence_claim: EQUIVALENCE_CLAIM,
        checks: report
            .checks
            .into_iter()
            .map(|check| ReadinessCheckBody {
                name: check.name,
                passed: check.passed,
                detail: check.detail,
            })
            .collect(),
    };
    let status = if report.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

const MARGIN_CALCULATION: &str = "\
SPAN margin is the greatest scenario loss in the archived NSE SPAN risk arrays, plus the calendar-spread charge from that file, plus a short-option minimum of zero when the file's rate is zero. \
A loss is positive for a long unit. Extreme loss margin uses the percent in the contract-wise ELM file. A futures spread charges one third of that percent on the far-month quantity and the full percent on the unmatched quantity. \
A short option is charged on the SPAN underlying price. Delivery margin on a stock derivative is 10, 25, 45, or 70 percent of the archived cash-market daily margin rate from four weekdays before expiry through the day before expiry, and the full rate on the expiry day. \
The total is SPAN margin plus extreme loss margin plus delivery margin, and a short option's premium is included. The SPAN file is the latest snapshot whose created time is at or before the trade datetime. That clock is the exchange time in the file, India Standard Time. Lot size comes from the NSE F&O contract file and is reported beside the line. Quantity stays in units. Amounts are rounded half away from zero to paise. \
This description is the quote method. It is not a reconciliation to NSE Clearing or ICCL.";

#[derive(Serialize)]
struct DataStatusBody {
    /// `none` until a quote is reconciled to a clearing-corporation reference.
    equivalence_claim: &'static str,
    /// Why the equivalence claim has its current value.
    statement: &'static str,
    /// How `POST /api/v1/margin/calculate` values a book. This text is the quote method, not a reconciliation result.
    margin_calculation: &'static str,
    /// `false` until the official method is implemented. `/ready` stays unavailable while this is false.
    official_calculation_implemented: bool,
    /// Published datasets with origin `official` and validation `passed`. A synthetic dataset is excluded.
    official_validated_datasets: u64,
}

async fn data_status(State(state): State<AppState>) -> Response {
    match state.store.validated_dataset_count().await {
        Ok(count) => (
            StatusCode::OK,
            Json(DataStatusBody {
                equivalence_claim: EQUIVALENCE_CLAIM,
                statement: EQUIVALENCE_STATEMENT,
                margin_calculation: MARGIN_CALCULATION,
                official_calculation_implemented: OFFICIAL_CALCULATION_IMPLEMENTED,
                official_validated_datasets: count,
            }),
        )
            .into_response(),
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "database_unavailable",
            error.to_string(),
        ),
    }
}

#[derive(Serialize)]
struct DatasetListBody {
    /// `none` until an official calculation is reconciled.
    equivalence_claim: &'static str,
    /// `true` when more than 500 datasets exist and this page stopped at 500.
    truncated: bool,
    /// Newest trading dates first.
    datasets: Vec<DatasetBody>,
}

#[derive(Serialize)]
struct DatasetEnvelope {
    /// `none` until an official calculation is reconciled.
    equivalence_claim: &'static str,
    /// Dataset metadata. The file bytes are not included.
    dataset: DatasetBody,
}

#[derive(Serialize)]
struct DatasetBody {
    /// Dataset UUID. Pass this to `GET /api/v1/risk-data/datasets/{dataset_id}`.
    id: String,
    /// `NSE` or `BSE`.
    exchange: String,
    /// `NSCCL` for NSE Clearing, or `ICCL`.
    clearing_corporation: String,
    /// Business date of the file, `YYYY-MM-DD`.
    trading_date: String,
    /// `begin_of_day`, `intraday`, or `end_of_day`.
    snapshot_class: String,
    /// Intraday sequence `1` through `4`. Absent for begin-of-day and end-of-day.
    intraday_sequence: Option<u32>,
    /// `pending`, `passed`, or `failed`.
    validation_status: String,
    /// `draft`, `published`, `superseded`, or `rejected`.
    publication_status: String,
    /// `official` or `synthetic`. A synthetic row is excluded from the official dataset count.
    origin: String,
    /// SHA-256 of the archived bytes.
    sha256: String,
    /// File layout, such as `nse-span-xml-4.00-zip`.
    file_format: String,
    /// Parser that accepted the file, or `none` when the bytes are stored only.
    parser_version: String,
    /// Filename as published, such as `nsccl.20261028.s.zip`.
    original_filename: String,
    /// Present when validation failed.
    validation_error_summary: Option<String>,
    /// Contract-master version linked to a published dataset. Absent on a draft SPAN file.
    contract_master_version_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[schema(
    description = "Failed request.",
    example = json!({
        "error": {
            "code": "invalid_request",
            "message": "at least one position is required"
        }
    })
)]
pub(crate) struct ErrorBody {
    /// Error code and message.
    error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
#[schema(description = "One API error. `code` is stable. `message` explains this occurrence.")]
struct ErrorDetail {
    /// `invalid_request`, `span_file_not_found`, `margin_rejected`, `download_failed`, `database_unavailable`, `invalid_dataset_id`, or `not_found`.
    #[schema(example = "invalid_request")]
    code: &'static str,
    /// Explanation safe to show to the caller.
    #[schema(example = "at least one position is required")]
    message: String,
}

async fn list_datasets(State(state): State<AppState>) -> Response {
    match state.store.list_datasets().await {
        Ok(page) => (
            StatusCode::OK,
            Json(DatasetListBody {
                equivalence_claim: EQUIVALENCE_CLAIM,
                truncated: page.truncated,
                datasets: page.datasets.into_iter().map(dataset_body).collect(),
            }),
        )
            .into_response(),
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "database_unavailable",
            error.to_string(),
        ),
    }
}

async fn get_dataset(State(state): State<AppState>, Path(dataset_id): Path<String>) -> Response {
    let Ok(uuid) = Uuid::parse_str(&dataset_id) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_dataset_id",
            "dataset id must be a UUID".to_owned(),
        );
    };
    match state.store.get_dataset(DatasetId::from_uuid(uuid)).await {
        Ok(Some(dataset)) => (
            StatusCode::OK,
            Json(DatasetEnvelope {
                equivalence_claim: EQUIVALENCE_CLAIM,
                dataset: dataset_body(margin_data::summarize(&dataset)),
            }),
        )
            .into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "risk dataset was not found".to_owned(),
        ),
        Err(error) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "database_unavailable",
            error.to_string(),
        ),
    }
}

fn dataset_body(summary: margin_data::DatasetSummary) -> DatasetBody {
    DatasetBody {
        id: summary.id.to_string(),
        exchange: summary.exchange.code().to_owned(),
        clearing_corporation: summary.clearing_corporation.code().to_owned(),
        trading_date: summary.trading_date.to_string(),
        snapshot_class: summary.snapshot_class.code().to_owned(),
        intraday_sequence: summary.intraday_sequence,
        validation_status: summary.validation_status.code().to_owned(),
        publication_status: summary.publication_status,
        origin: summary.origin.code().to_owned(),
        sha256: summary.sha256,
        file_format: summary.file_format,
        parser_version: summary.parser_version,
        original_filename: summary.original_filename,
        validation_error_summary: summary.validation_error_summary,
        contract_master_version_id: summary.contract_master_version_id.map(|id| id.to_string()),
    }
}

pub(crate) fn error_response(status: StatusCode, code: &'static str, message: String) -> Response {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail { code, message },
        }),
    )
        .into_response()
}
