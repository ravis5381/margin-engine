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
use margin_data::{AppStore, DatasetReadModel};
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

#[derive(Serialize, ToSchema)]
#[schema(description = "Liveness payload. It does not report database or calculation readiness.")]
struct HealthBody {
    status: &'static str,
    version: &'static str,
    equivalence_claim: &'static str,
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "operations",
    operation_id = "health",
    summary = "Liveness",
    description = "Reports that the process is answering. This check does not query the database and does not claim a margin calculation is available.",
    responses(
        (status = 200, description = "Process is alive", body = HealthBody)
    )
)]
async fn health() -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        equivalence_claim: EQUIVALENCE_CLAIM,
    })
}

#[derive(Serialize, ToSchema)]
#[schema(description = "Readiness payload. status is ready only when every check passed.")]
struct ReadinessBody {
    status: &'static str,
    equivalence_claim: &'static str,
    checks: Vec<ReadinessCheckBody>,
}

#[derive(Serialize, ToSchema)]
struct ReadinessCheckBody {
    name: &'static str,
    passed: bool,
    detail: String,
}

#[utoipa::path(
    get,
    path = "/ready",
    tag = "operations",
    operation_id = "ready",
    summary = "Readiness",
    description = "Returns 200 only when the database responds, at least one published official dataset is stored, and an official margin calculation is implemented. This build does not implement that calculation, so the official_calculation check fails.",
    responses(
        (status = 200, description = "Every readiness check passed", body = ReadinessBody),
        (status = 503, description = "At least one readiness check failed", body = ReadinessBody)
    )
)]
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

#[derive(Serialize, ToSchema)]
#[schema(description = "Official-data status. Synthetic datasets are excluded from the count.")]
struct DataStatusBody {
    equivalence_claim: &'static str,
    statement: &'static str,
    official_calculation_implemented: bool,
    official_validated_datasets: u64,
}

#[utoipa::path(
    get,
    path = "/api/v1/data/status",
    tag = "operations",
    operation_id = "data_status",
    summary = "Official data status",
    description = "Reports whether an official margin calculation is implemented and how many published official datasets are stored. A synthetic dataset does not increase the count.",
    responses(
        (status = 200, description = "Status was read", body = DataStatusBody),
        (status = 503, description = "Dataset status could not be read", body = ErrorBody)
    )
)]
async fn data_status(State(state): State<AppState>) -> Response {
    match state.store.validated_dataset_count().await {
        Ok(count) => (
            StatusCode::OK,
            Json(DataStatusBody {
                equivalence_claim: EQUIVALENCE_CLAIM,
                statement: EQUIVALENCE_STATEMENT,
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

#[derive(Serialize, ToSchema)]
#[schema(
    description = "Dataset metadata page. The list is capped at 500 rows and contains no margin amounts."
)]
struct DatasetListBody {
    equivalence_claim: &'static str,
    truncated: bool,
    datasets: Vec<DatasetBody>,
}

#[derive(Serialize, ToSchema)]
struct DatasetEnvelope {
    equivalence_claim: &'static str,
    dataset: DatasetBody,
}

#[derive(Serialize, ToSchema)]
#[schema(description = "Archived risk-dataset metadata. Source URLs are omitted.")]
struct DatasetBody {
    id: String,
    exchange: String,
    clearing_corporation: String,
    trading_date: String,
    snapshot_class: String,
    intraday_sequence: Option<u32>,
    validation_status: String,
    publication_status: String,
    origin: String,
    sha256: String,
    file_format: String,
    parser_version: String,
    original_filename: String,
    validation_error_summary: Option<String>,
    contract_master_version_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/risk-data/datasets",
    tag = "risk-data",
    operation_id = "list_datasets",
    summary = "List risk datasets",
    description = "Lists archived dataset metadata. At most 500 rows are returned, and truncated is true when more rows exist. The payload does not include margin amounts or source URLs.",
    responses(
        (status = 200, description = "Dataset metadata page", body = DatasetListBody),
        (status = 503, description = "Dataset list could not be read", body = ErrorBody)
    )
)]
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

#[utoipa::path(
    get,
    path = "/api/v1/risk-data/datasets/{dataset_id}",
    tag = "risk-data",
    operation_id = "get_dataset",
    summary = "Get one risk dataset",
    description = "Returns metadata for one archived dataset. The identifier is the dataset UUID, not a display symbol.",
    params(
        ("dataset_id" = String, Path, description = "UUID of an archived risk dataset")
    ),
    responses(
        (status = 200, description = "Dataset metadata", body = DatasetEnvelope),
        (status = 400, description = "dataset_id is not a UUID", body = ErrorBody),
        (status = 404, description = "No dataset has that id", body = ErrorBody),
        (status = 503, description = "Dataset could not be read", body = ErrorBody)
    )
)]
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
