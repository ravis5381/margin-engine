mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{DatasetParts, dataset};
use http_body_util::BodyExt;
use margin_api::{AppState, router};
use margin_data::{AppStore, MemoryStore};
use margin_domain::{ClearingCorporation, DatasetOrigin, Exchange};
use std::time::Duration;
use tower::ServiceExt;

fn state(datasets: Vec<margin_domain::RiskDataset>) -> AppState {
    AppState {
        store: AppStore::Memory(MemoryStore::new(datasets)),
        body_limit: 2048,
        request_timeout: Duration::from_secs(2),
        max_concurrency: 4,
    }
}

async fn call(
    datasets: Vec<margin_domain::RiskDataset>,
    method: &str,
    uri: &str,
) -> (StatusCode, serde_json::Value) {
    let response = router(state(datasets))
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn health_is_public_and_ready_does_not_claim_a_calculation_engine() {
    let (status, body) = call(Vec::new(), "GET", "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["equivalence_claim"], "none");

    let (status, body) = call(Vec::new(), "GET", "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "not_ready");
    assert_eq!(body["equivalence_claim"], "none");
    let calculation = body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == "official_calculation")
        .expect("calculation check");
    assert_eq!(calculation["passed"], false);
}

#[tokio::test]
async fn an_official_dataset_does_not_make_the_service_ready() {
    let mut parts = DatasetParts::published(Exchange::Bse, ClearingCorporation::Iccl, 9, 30);
    parts.origin = DatasetOrigin::Official;
    parts.source_host = Some("www.icclindia.com".to_owned());
    let official = dataset(parts);
    let (status, body) = call(vec![official], "GET", "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let dataset_check = body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == "official_validated_dataset")
        .expect("dataset check");
    assert_eq!(dataset_check["passed"], true);
    let (status, body) = call(Vec::new(), "GET", "/api/v1/data/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["official_calculation_implemented"], false);
    assert_eq!(body["official_validated_datasets"], 0);
    let synthetic = dataset(DatasetParts::published(
        Exchange::Nse,
        ClearingCorporation::Nscc,
        2,
        33,
    ));
    let (status, body) = call(vec![synthetic], "GET", "/api/v1/data/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["official_validated_datasets"], 0);
    assert!(
        body["statement"]
            .as_str()
            .expect("statement")
            .contains("does not replicate")
    );
}

#[tokio::test]
async fn dataset_metadata_is_listed_without_a_margin_amount() {
    let synthetic = dataset(DatasetParts::published(
        Exchange::Nse,
        ClearingCorporation::Nscc,
        4,
        31,
    ));
    let id = synthetic.id().to_string();
    let (status, body) = call(vec![synthetic], "GET", "/api/v1/risk-data/datasets").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["equivalence_claim"], "none");
    assert_eq!(body["datasets"][0]["id"], id);
    assert!(body["datasets"][0].get("total_margin").is_none());

    let (status, _body) = call(
        vec![dataset(DatasetParts::published(
            Exchange::Nse,
            ClearingCorporation::Nscc,
            4,
            32,
        ))],
        "GET",
        &format!("/api/v1/risk-data/datasets/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(Vec::new(), "GET", "/api/v1/risk-data/datasets/not-a-uuid").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = call(Vec::new(), "POST", "/api/v1/margin/calculate").await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(body.get("span_margin").is_none());
    assert!(body.get("total_margin").is_none());
}

#[tokio::test]
async fn openapi_documents_published_routes_and_swagger_ui_loads() {
    let (status, body) = call(Vec::new(), "GET", "/api/v1/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["openapi"]
            .as_str()
            .expect("openapi version")
            .starts_with("3.")
    );
    assert!(
        body["info"]["description"]
            .as_str()
            .expect("description")
            .contains("does not replicate")
    );
    let paths = body["paths"].as_object().expect("paths");
    assert!(paths.contains_key("/health"));
    assert!(paths.contains_key("/ready"));
    assert!(paths.contains_key("/api/v1/data/status"));
    assert!(paths.contains_key("/api/v1/risk-data/datasets"));
    assert!(paths.contains_key("/api/v1/risk-data/datasets/{dataset_id}"));
    assert!(paths.contains_key("/api/v1/margin/calculate"));
    assert!(!paths.contains_key("/api/v1/risk-data/ingest"));
    assert_eq!(paths.len(), 6);

    let response = router(state(Vec::new()))
        .oneshot(
            Request::builder()
                .uri("/swagger-ui/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let html = String::from_utf8(bytes.to_vec()).expect("html");
    assert!(html.contains("swagger-ui"));
    assert!(html.contains("swagger-initializer.js"));
    assert!(html.contains(".opblock-section.operation-servers{display:none}"));

    let initializer = router(state(Vec::new()))
        .oneshot(
            Request::builder()
                .uri("/swagger-ui/swagger-initializer.js")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(initializer.status(), StatusCode::OK);
    let script = String::from_utf8(
        initializer
            .into_body()
            .collect()
            .await
            .expect("script")
            .to_bytes()
            .to_vec(),
    )
    .expect("script");
    assert!(script.contains("/api/v1/openapi.json"));
}

#[tokio::test]
async fn unavailable_database_is_not_ready_and_lists_no_data() {
    let state = AppState {
        store: AppStore::Unavailable {
            reason: "DATABASE_URL is not set".to_owned(),
        },
        body_limit: 1024,
        request_timeout: Duration::from_secs(1),
        max_concurrency: 2,
    };
    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/data/status")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
