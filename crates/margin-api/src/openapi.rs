use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use utoipa::OpenApi;

use crate::margin::{
    CanonicalInstrumentRequest, CanonicalInstrumentResponse, CombinedMarginBody, MarginRequest,
    MarginResponse, PositionMarginBody,
};
use crate::{ErrorBody, ErrorDetail};

const DESCRIPTION: &str = "\
Quote the margin for an NSE or BSE futures and options book.\n\n\
1. Build a symbol with **Canonical symbol**, or skip that and send the contract fields on the margin call.\n\
2. Call **Quote margin**. The reply has the combined margin and one row per position.\n\n\
Amounts are decimal strings in INR, rounded half away from zero to paise. A negative quantity is a short. A missing file is an error. A calculated zero is `0`.\n\n\
This API does not replicate NSE Clearing or ICCL.";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Margin calculator",
        description = DESCRIPTION,
        version = env!("CARGO_PKG_VERSION")
    ),
    paths(
        crate::margin::calculate_margin,
        crate::margin::canonical_instrument
    ),
    components(schemas(
        ErrorBody,
        ErrorDetail,
        MarginRequest,
        MarginResponse,
        CombinedMarginBody,
        PositionMarginBody,
        CanonicalInstrumentRequest,
        CanonicalInstrumentResponse
    )),
    tags(
        (name = "margin", description = "SPAN margin, exposure margin, premium, and the total for one book."),
        (name = "instruments", description = "Build the symbol the margin call matches. This does not return a margin.")
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
        .filter(false)
        .try_it_out_enabled(true)
        .display_request_duration(true)
        .default_models_expand_depth(-1)
        .default_model_expand_depth(1)
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
.swagger-ui .topbar .link::after{content:\"Margin calculator\";color:#f8fafc;font:600 15px/1 \"Segoe UI\",system-ui,sans-serif}\
.swagger-ui .info__license,.swagger-ui .info__contact{display:none}\
.swagger-ui .information-container{padding:16px 0 8px}\
.swagger-ui section.models{display:none}\
.swagger-ui .scheme-container{display:none}\
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
