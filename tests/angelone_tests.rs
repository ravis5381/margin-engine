//! Replies captured from Angel One's public margin calculator for the NSE
//! session of 2026-10-28. The trade time 18:30 IST is the SPAN file that
//! calculator was using on the morning of 2026-10-29. The test does not call
//! Angel One. It needs the archived files for that session in PostgreSQL.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use margin_api::{AppState, router};
use margin_data::{AppStore, PgStore, connect};
use std::time::Duration;
use tower::ServiceExt;

const TRADING_DATETIME: &str = "2026-10-28T18:30:00+05:30";

const SHORT_STRANGLE: &str = r#"{
  "margin": {
    "netPremium": "-107568",
    "SPANMargin": "238455.6",
    "exposureMargin": "63775.68",
    "elm": "63775.68",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "124515.6",
    "totalMargin": "302231.28"
  },
  "totalPositionMargin": "426746.88",
  "positionMargin": [
    {
      "exchange": "NSE",
      "contract": "NSE:MIDCPNIFTY:2026-11-23:13700:CE",
      "product": "OPTION",
      "strikePrice": "13700",
      "instrumentType": "CE",
      "tradeType": "SELL",
      "optionType": "CE",
      "qty": "120",
      "premium": "-17574",
      "SPANMargin": "124515.6",
      "exposureMargin": "31887.84",
      "elm": "31887.84",
      "totalMargin": "156403.44"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:MIDCPNIFTY:2026-11-23:14000:PE",
      "product": "OPTION",
      "strikePrice": "14000",
      "instrumentType": "PE",
      "tradeType": "SELL",
      "optionType": "PE",
      "qty": "120",
      "premium": "-89994",
      "SPANMargin": "238455.6",
      "exposureMargin": "31887.84",
      "elm": "31887.84",
      "totalMargin": "270343.44"
    }
  ]
}"#;

const LONG_STRANGLE: &str = r#"{
  "margin": {
    "netPremium": "107568",
    "SPANMargin": "0",
    "exposureMargin": "0",
    "elm": "0",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "0",
    "totalMargin": "0"
  },
  "totalPositionMargin": "0",
  "positionMargin": [
    {
      "exchange": "NSE",
      "contract": "NSE:MIDCPNIFTY:2026-11-23:13700:CE",
      "product": "OPTION",
      "strikePrice": "13700",
      "instrumentType": "CE",
      "tradeType": "BUY",
      "optionType": "CE",
      "qty": "120",
      "premium": "17574",
      "SPANMargin": "0",
      "exposureMargin": "0",
      "elm": "0",
      "totalMargin": "0"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:MIDCPNIFTY:2026-11-23:14000:PE",
      "product": "OPTION",
      "strikePrice": "14000",
      "instrumentType": "PE",
      "tradeType": "BUY",
      "optionType": "PE",
      "qty": "120",
      "premium": "89994",
      "SPANMargin": "0",
      "exposureMargin": "0",
      "elm": "0",
      "totalMargin": "0"
    }
  ]
}"#;

const LONG_STRANGLE_AND_SHORT_NIFTY_PUT: &str = r#"{
  "margin": {
    "netPremium": "3867",
    "SPANMargin": "305432.7",
    "exposureMargin": "59017.01",
    "elm": "59017.01",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "72496.8",
    "totalMargin": "364449.71"
  },
  "totalPositionMargin": "436946.51",
  "positionMargin": [
    {
      "exchange": "NSE",
      "contract": "NSE:MIDCPNIFTY:2026-11-23:13700:CE",
      "product": "OPTION",
      "strikePrice": "13700",
      "instrumentType": "CE",
      "tradeType": "BUY",
      "optionType": "CE",
      "qty": "120",
      "premium": "17574",
      "SPANMargin": "0",
      "exposureMargin": "0",
      "elm": "0",
      "totalMargin": "0"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:MIDCPNIFTY:2026-11-23:14000:PE",
      "product": "OPTION",
      "strikePrice": "14000",
      "instrumentType": "PE",
      "tradeType": "BUY",
      "optionType": "PE",
      "qty": "120",
      "premium": "89994",
      "SPANMargin": "0",
      "exposureMargin": "0",
      "elm": "0",
      "totalMargin": "0"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:NIFTY:2026-11-03:23500:PE",
      "product": "OPTION",
      "strikePrice": "23500",
      "instrumentType": "PE",
      "tradeType": "SELL",
      "optionType": "PE",
      "qty": "130",
      "premium": "-103701",
      "SPANMargin": "377929.5",
      "exposureMargin": "59017.01",
      "elm": "59017.01",
      "totalMargin": "436946.51"
    }
  ]
}"#;

#[tokio::test]
async fn session_2026_10_28_matches_captured_angel_one_replies() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL is unset; skipping Angel One reply test");
        return;
    };
    if url.trim().is_empty() {
        eprintln!("DATABASE_URL is empty; skipping Angel One reply test");
        return;
    }
    let pool = connect(&url, 2, Duration::from_secs(5))
        .await
        .expect("connect");
    let state = AppState {
        store: AppStore::Postgres(PgStore::new(pool)),
        config: None,
        body_limit: 8192,
        request_timeout: Duration::from_secs(180),
        max_concurrency: 2,
    };
    let books = [
        (
            "short MIDCPNIFTY 13700 call and 14000 put",
            r#"[{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"13700","right":"CE","quantity":"-120"},{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"14000","right":"PE","quantity":"-120"}]"#,
            SHORT_STRANGLE,
        ),
        (
            "long MIDCPNIFTY 13700 call and 14000 put",
            r#"[{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"13700","right":"CE","quantity":"120"},{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"14000","right":"PE","quantity":"120"}]"#,
            LONG_STRANGLE,
        ),
        (
            "long MIDCPNIFTY strangle and short NIFTY 23500 put",
            r#"[{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"13700","right":"CE","quantity":"120"},{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"14000","right":"PE","quantity":"120"},{"underlying":"NIFTY","expiry":"2026-11-03","strike":"23500","right":"PE","quantity":"-130"}]"#,
            LONG_STRANGLE_AND_SHORT_NIFTY_PUT,
        ),
    ];
    for (name, positions, expected) in books {
        let body = format!(
            r#"{{"exchange":"NSE","trading_datetime":"{TRADING_DATETIME}","positions":{positions}}}"#
        );
        let actual = quote(&state, &body).await;
        let expected: serde_json::Value = serde_json::from_str(expected).expect(name);
        assert_eq!(actual, expected, "{name}");
    }
}

async fn quote(state: &AppState, body: &str) -> serde_json::Value {
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/margin/calculate")
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
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
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    assert_eq!(status, StatusCode::OK, "{json}");
    json
}
