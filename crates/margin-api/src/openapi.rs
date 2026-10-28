use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use utoipa::OpenApi;

use crate::margin::{
    CanonicalInstrumentRequest, CanonicalInstrumentResponse, MarginLineBody, MarginRequest,
    MarginResponse,
};
use crate::{
    DataStatusBody, DatasetBody, DatasetEnvelope, DatasetListBody, ErrorBody, ErrorDetail,
    HealthBody, ReadinessBody, ReadinessCheckBody,
};

const DESCRIPTION: &str = "\
HTTP API for liveness, readiness, archived risk-dataset metadata, and SPAN margin.\n\n\
This build does not replicate NSE Clearing or ICCL margin calculations. \
The margin route takes a trade datetime and returns scanning risk, the calendar-spread charge, extreme loss margin, option premium, and delivery margin for futures, options, and physical stocks from the SPAN file current at that time. \
Cross-margin benefit is zero. \
Reported amounts are rounded half away from zero to paise. \
POST /api/v1/instruments/canonical builds the canonical instrument and does not return a margin. \
Ingestion and reconciliation routes are not published. \
The diagnostic scenario sum is not an API response and is not a margin.";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Margin engine",
        description = DESCRIPTION,
        version = env!("CARGO_PKG_VERSION")
    ),
    paths(
        crate::health,
        crate::ready,
        crate::data_status,
        crate::list_datasets,
        crate::get_dataset,
        crate::margin::calculate_margin,
        crate::margin::canonical_instrument
    ),
    components(schemas(
        HealthBody,
        ReadinessBody,
        ReadinessCheckBody,
        DataStatusBody,
        DatasetListBody,
        DatasetEnvelope,
        DatasetBody,
        ErrorBody,
        ErrorDetail,
        MarginRequest,
        MarginResponse,
        MarginLineBody,
        CanonicalInstrumentRequest,
        CanonicalInstrumentResponse
    )),
    tags(
        (name = "operations", description = "Liveness, readiness, and official-data status. These routes do not calculate margin."),
        (name = "risk-data", description = "Archived risk-dataset metadata. Responses contain no margin amounts."),
        (name = "margin", description = "SPAN scanning risk, calendar-spread charge, extreme loss margin, option premium, and delivery margin. Cross-margin benefit is zero."),
        (name = "instruments", description = "Canonical instrument symbols built from exchange, underlying, expiry, strike, and right. These routes do not calculate margin.")
    )
)]
struct ApiDoc;

pub(super) fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let swagger: Router<S> = utoipa_swagger_ui::SwaggerUi::new("/swagger-ui")
        .url("/api/v1/openapi.json", ApiDoc::openapi())
        .into();
    swagger.layer(middleware::from_fn(hide_operation_servers))
}

const HIDE_SERVERS: &str = "<style>.opblock-section.operation-servers{display:none}</style>";

async fn hide_operation_servers(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let response = next.run(request).await;
    if !swagger_index(&path) {
        return response;
    }
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("text/html") {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 1_048_576).await else {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "swagger page could not be read",
        )
            .into_response();
    };
    let Ok(mut html) = String::from_utf8(bytes.to_vec()) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if let Some(index) = html.find("</head>") {
        html.insert_str(index, HIDE_SERVERS);
    } else {
        html.insert_str(0, HIDE_SERVERS);
    }
    if let Ok(length) = axum::http::HeaderValue::from_str(&html.len().to_string()) {
        parts
            .headers
            .insert(axum::http::header::CONTENT_LENGTH, length);
    }
    Response::from_parts(parts, Body::from(html))
}

fn swagger_index(path: &str) -> bool {
    path == "/swagger-ui" || path == "/swagger-ui/" || path.ends_with("/index.html")
}
