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
Quote initial margin for an NSE equity-derivatives book from the SPAN file that was current at a trade time.\n\n\
## Conventions\n\n\
- Amounts are decimal strings in INR. Each reported amount is rounded half away from zero to paise. Trailing zeros may be omitted.\n\
- Quantity is in units. A negative quantity is a short position. Lot size is reported on the line and does not rescale the quantity or the margin.\n\
- A `trading_datetime` with no offset is India Standard Time (`+05:30`), the clock in the SPAN `created` element. An RFC 3339 value with an offset is converted to that clock.\n\
- Canonical symbols are `NSE:UNDERLYING:YYYY-MM-DD:FUT`, `NSE:UNDERLYING:YYYY-MM-DD:STRIKE:CE`, `NSE:UNDERLYING:YYYY-MM-DD:STRIKE:PE`, and `NSE:UNDERLYING:EQ`.\n\
- A position sends either a symbol or the contract fields. Sending both is rejected before the archive is read.\n\
- Errors use `{ \"error\": { \"code\": \"...\", \"message\": \"...\" } }`. A missing file or contract is an error. A calculated zero is returned when the method evaluates to zero.\n\n\
## Errors\n\n\
| Status | Code | Meaning |\n\
| --- | --- | --- |\n\
| 400 | `invalid_dataset_id` | `dataset_id` is not a UUID. |\n\
| 404 | `span_file_not_found` | No SPAN file was published at or before the trade time. |\n\
| 404 | `not_found` | No archived dataset has that id. |\n\
| 408 | | The quote exceeded the configured request timeout. The body is empty. |\n\
| 422 | `invalid_request` | The book or instrument fields are not acceptable. |\n\
| 422 | `margin_rejected` | An archived file cannot value this book. |\n\
| 422 | `download_failed` | A required official file could not be archived. |\n\
| 503 | `database_unavailable` | The archive could not be read. |\n\n\
## Scope\n\n\
This API does not replicate NSE Clearing or ICCL. `equivalence_claim` stays `none` until a calculated total matches a clearing-corporation reference for the same book and file. `/ready` stays unavailable while that claim is open. Ingestion and reconciliation are not published.";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Margin engine",
        description = DESCRIPTION,
        version = env!("CARGO_PKG_VERSION"),
        license(name = "UNLICENSED")
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
        (name = "margin", description = "Quote SPAN, extreme loss, premium, and delivery margin for one NSE book. The trade time selects the SPAN snapshot."),
        (name = "instruments", description = "Build a canonical symbol from exchange, underlying, expiry, strike, and right. These routes do not read the archive and do not return a margin."),
        (name = "risk-data", description = "Metadata for archived risk files. Responses contain hashes and status, not margin amounts or source URLs."),
        (name = "operations", description = "Liveness, readiness, and the claim gate for an official calculation. These routes do not value a book.")
    )
)]
struct ApiDoc;

pub(super) fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let config = utoipa_swagger_ui::Config::default()
        .deep_linking(true)
        .doc_expansion("list")
        .filter(true)
        .try_it_out_enabled(true)
        .display_request_duration(true)
        .default_models_expand_depth(1)
        .default_model_expand_depth(2)
        .default_model_rendering("example")
        .validator_url("none");
    let swagger: Router<S> = utoipa_swagger_ui::SwaggerUi::new("/swagger-ui")
        .url("/api/v1/openapi.json", ApiDoc::openapi())
        .config(config)
        .into();
    swagger.layer(middleware::from_fn(hide_operation_servers))
}

const HIDE_SERVERS: &str = "<style>\
.opblock-section.operation-servers{display:none}\
.swagger-ui .topbar{background:#111827;border-bottom:2px solid #0f766e;padding:8px 0}\
.swagger-ui .topbar .download-url-wrapper{display:none}\
.swagger-ui .topbar .link svg,.swagger-ui .topbar .link img{display:none}\
.swagger-ui .topbar .link::after{content:\"Margin engine\";color:#f8fafc;font:600 15px/1 \"Segoe UI\",system-ui,sans-serif}\
.swagger-ui .info .title{font-weight:600}\
.swagger-ui .info .title small{background:#ecfdf5;color:#115e59}\
.swagger-ui .opblock-tag{border-bottom:1px solid #e5e7eb;font-size:15px}\
.swagger-ui .opblock.opblock-post{border-color:#0f766e;background:rgba(15,118,110,.04)}\
.swagger-ui .opblock.opblock-post .opblock-summary-method{background:#0f766e}\
.swagger-ui .btn.execute{background:#0f766e;border-color:#0f766e}\
.swagger-ui .renderedMarkdown p,.swagger-ui .markdown p{line-height:1.55}\
.swagger-ui .filter .operation-filter-input{border:1px solid #d1d5db;border-radius:6px;padding:6px 8px}\
</style>";

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
