//! Replies checked against Angel One's public margin calculator for the NSE
//! session of 2026-10-28. The 18:30 IST trade time is the SPAN file that
//! calculator was using for NSE on the morning of 2026-10-29. Its BSE SENSEX
//! future was still on the morning file: SPAN matches at 10:00 IST, and the
//! exposure here is 2 percent of the futures settlement. SENSEX options at
//! that same file use the IO percent and the SPAN index price. The November
//! 74000 call and 75000 put match Angel One. The 29 October 74000 call and
//! 73000 put match on SPAN and premium. Their short exposure here is 29439.47,
//! the file's 2 percent of index 73598.67. Angel One showed 58878.94, 4 percent
//! of that index: the file still says 2.00, and the extra 2 points are the
//! expiry-day charge on Angel One's calculator date of 29 October. A long
//! option of either expiry matches Angel One. The test does not
//! call Angel One. It needs the archived files for that session in PostgreSQL.
//! The HDFCBANK 700 call exposure is 15590.58. Angel One showed 15590.57; the
//! unrounded charge is 15590.575 and this quote rounds half away from zero.

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

const SENSEX_NOVEMBER_FUTURE: &str = r#"{
  "margin": {
    "netPremium": "0",
    "SPANMargin": "136974.8",
    "exposureMargin": "29620.78",
    "elm": "29620.78",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "0",
    "totalMargin": "166595.58"
  },
  "totalPositionMargin": "166595.58",
  "positionMargin": [
    {
      "exchange": "BSE",
      "contract": "BSE:SENSEX:2026-11-26:FUT",
      "product": "FUTURE",
      "strikePrice": "0",
      "instrumentType": "FUT",
      "tradeType": "SELL",
      "optionType": "",
      "qty": "20",
      "premium": "0",
      "SPANMargin": "136974.8",
      "exposureMargin": "29620.78",
      "elm": "29620.78",
      "totalMargin": "166595.58"
    }
  ]
}"#;

const SENSEX_OCTOBER_CALL_SHORT: &str = r#"{
  "margin": {
    "netPremium": "-4567",
    "SPANMargin": "129802.8",
    "exposureMargin": "29439.47",
    "elm": "29439.47",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "0",
    "totalMargin": "159242.27"
  },
  "totalPositionMargin": "159242.27",
  "positionMargin": [
    {
      "exchange": "BSE",
      "contract": "BSE:SENSEX:2026-10-29:74000:CE",
      "product": "OPTION",
      "strikePrice": "74000",
      "instrumentType": "CE",
      "tradeType": "SELL",
      "optionType": "CE",
      "qty": "20",
      "premium": "-4567",
      "SPANMargin": "129802.8",
      "exposureMargin": "29439.47",
      "elm": "29439.47",
      "totalMargin": "159242.27"
    }
  ]
}"#;

const SENSEX_OCTOBER_PUT_SHORT: &str = r#"{
  "margin": {
    "netPremium": "-3832",
    "SPANMargin": "126089",
    "exposureMargin": "29439.47",
    "elm": "29439.47",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "0",
    "totalMargin": "155528.47"
  },
  "totalPositionMargin": "155528.47",
  "positionMargin": [
    {
      "exchange": "BSE",
      "contract": "BSE:SENSEX:2026-10-29:73000:PE",
      "product": "OPTION",
      "strikePrice": "73000",
      "instrumentType": "PE",
      "tradeType": "SELL",
      "optionType": "PE",
      "qty": "20",
      "premium": "-3832",
      "SPANMargin": "126089",
      "exposureMargin": "29439.47",
      "elm": "29439.47",
      "totalMargin": "155528.47"
    }
  ]
}"#;

const SENSEX_OCTOBER_CALL_LONG: &str = r#"{
  "margin": {
    "netPremium": "4567",
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
      "exchange": "BSE",
      "contract": "BSE:SENSEX:2026-10-29:74000:CE",
      "product": "OPTION",
      "strikePrice": "74000",
      "instrumentType": "CE",
      "tradeType": "BUY",
      "optionType": "CE",
      "qty": "20",
      "premium": "4567",
      "SPANMargin": "0",
      "exposureMargin": "0",
      "elm": "0",
      "totalMargin": "0"
    }
  ]
}"#;

const SENSEX_NOVEMBER_CALL_SHORT: &str = r#"{
  "margin": {
    "netPremium": "-25946",
    "SPANMargin": "137209.6",
    "exposureMargin": "29439.47",
    "elm": "29439.47",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "0",
    "totalMargin": "166649.07"
  },
  "totalPositionMargin": "166649.07",
  "positionMargin": [
    {
      "exchange": "BSE",
      "contract": "BSE:SENSEX:2026-11-26:74000:CE",
      "product": "OPTION",
      "strikePrice": "74000",
      "instrumentType": "CE",
      "tradeType": "SELL",
      "optionType": "CE",
      "qty": "20",
      "premium": "-25946",
      "SPANMargin": "137209.6",
      "exposureMargin": "29439.47",
      "elm": "29439.47",
      "totalMargin": "166649.07"
    }
  ]
}"#;

const SENSEX_NOVEMBER_PUT_SHORT: &str = r#"{
  "margin": {
    "netPremium": "-35428",
    "SPANMargin": "154916.2",
    "exposureMargin": "29439.47",
    "elm": "29439.47",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "0",
    "totalMargin": "184355.67"
  },
  "totalPositionMargin": "184355.67",
  "positionMargin": [
    {
      "exchange": "BSE",
      "contract": "BSE:SENSEX:2026-11-26:75000:PE",
      "product": "OPTION",
      "strikePrice": "75000",
      "instrumentType": "PE",
      "tradeType": "SELL",
      "optionType": "PE",
      "qty": "20",
      "premium": "-35428",
      "SPANMargin": "154916.2",
      "exposureMargin": "29439.47",
      "elm": "29439.47",
      "totalMargin": "184355.67"
    }
  ]
}"#;

const SENSEX_NOVEMBER_PUT_LONG: &str = r#"{
  "margin": {
    "netPremium": "35428",
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
      "exchange": "BSE",
      "contract": "BSE:SENSEX:2026-11-26:75000:PE",
      "product": "OPTION",
      "strikePrice": "75000",
      "instrumentType": "PE",
      "tradeType": "BUY",
      "optionType": "PE",
      "qty": "20",
      "premium": "35428",
      "SPANMargin": "0",
      "exposureMargin": "0",
      "elm": "0",
      "totalMargin": "0"
    }
  ]
}"#;

const STOCK_FUTURES_AND_OPTIONS: &str = r#"{
  "margin": {
    "netPremium": "-220",
    "SPANMargin": "273998.5",
    "exposureMargin": "85826.57",
    "elm": "85826.57",
    "deliveryMargin": "0",
    "additionalMargin": "0",
    "tenderMargin": "0",
    "specialMargin": "0",
    "additionalPreExpiryMargin": "0",
    "marginBenefit": "81284",
    "totalMargin": "359825.07"
  },
  "totalPositionMargin": "441109.07",
  "positionMargin": [
    {
      "exchange": "NSE",
      "contract": "NSE:HDFCBANK:2026-11-23:FUT",
      "product": "FUTURE",
      "strikePrice": "0",
      "instrumentType": "FUT",
      "tradeType": "SELL",
      "optionType": "",
      "qty": "650",
      "premium": "0",
      "SPANMargin": "63368.5",
      "exposureMargin": "15632.66",
      "elm": "15632.66",
      "totalMargin": "79001.16"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:HDFCBANK:2026-11-23:700:CE",
      "product": "OPTION",
      "strikePrice": "700",
      "instrumentType": "CE",
      "tradeType": "SELL",
      "optionType": "CE",
      "qty": "650",
      "premium": "-6857.5",
      "SPANMargin": "55796",
      "exposureMargin": "15590.58",
      "elm": "15590.58",
      "totalMargin": "71386.58"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:TCS:2026-11-23:FUT",
      "product": "FUTURE",
      "strikePrice": "0",
      "instrumentType": "FUT",
      "tradeType": "BUY",
      "optionType": "",
      "qty": "225",
      "premium": "0",
      "SPANMargin": "85533.75",
      "exposureMargin": "19466.21",
      "elm": "19466.21",
      "totalMargin": "104999.96"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:TCS:2026-11-23:2440:PE",
      "product": "OPTION",
      "strikePrice": "2440",
      "instrumentType": "PE",
      "tradeType": "SELL",
      "optionType": "PE",
      "qty": "225",
      "premium": "-13882.5",
      "SPANMargin": "77384.25",
      "exposureMargin": "19372.42",
      "elm": "19372.42",
      "totalMargin": "96756.67"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:INFY:2026-11-23:FUT",
      "product": "FUTURE",
      "strikePrice": "0",
      "instrumentType": "FUT",
      "tradeType": "SELL",
      "optionType": "",
      "qty": "400",
      "premium": "0",
      "SPANMargin": "73200",
      "exposureMargin": "15764.7",
      "elm": "15764.7",
      "totalMargin": "88964.7"
    },
    {
      "exchange": "NSE",
      "contract": "NSE:INFY:2026-11-23:1100:CE",
      "product": "OPTION",
      "strikePrice": "1100",
      "instrumentType": "CE",
      "tradeType": "BUY",
      "optionType": "CE",
      "qty": "400",
      "premium": "20520",
      "SPANMargin": "0",
      "exposureMargin": "0",
      "elm": "0",
      "totalMargin": "0"
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
        body_limit: 16_384,
        request_timeout: Duration::from_secs(180),
        max_concurrency: 2,
    };
    let books = [
        (
            "short MIDCPNIFTY 13700 call and 14000 put",
            "NSE",
            TRADING_DATETIME,
            r#"[{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"13700","right":"CE","quantity":"-120"},{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"14000","right":"PE","quantity":"-120"}]"#,
            SHORT_STRANGLE,
        ),
        (
            "long MIDCPNIFTY 13700 call and 14000 put",
            "NSE",
            TRADING_DATETIME,
            r#"[{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"13700","right":"CE","quantity":"120"},{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"14000","right":"PE","quantity":"120"}]"#,
            LONG_STRANGLE,
        ),
        (
            "long MIDCPNIFTY strangle and short NIFTY 23500 put",
            "NSE",
            TRADING_DATETIME,
            r#"[{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"13700","right":"CE","quantity":"120"},{"underlying":"MIDCPNIFTY","expiry":"2026-11-23","strike":"14000","right":"PE","quantity":"120"},{"underlying":"NIFTY","expiry":"2026-11-03","strike":"23500","right":"PE","quantity":"-130"}]"#,
            LONG_STRANGLE_AND_SHORT_NIFTY_PUT,
        ),
        (
            "short SENSEX November future",
            "BSE",
            "2026-10-28T10:00:00+05:30",
            r#"[{"underlying":"SENSEX","expiry":"2026-11-26","right":"FUT","quantity":"-20"}]"#,
            SENSEX_NOVEMBER_FUTURE,
        ),
        (
            "short SENSEX 29 October 74000 call",
            "BSE",
            "2026-10-28T10:00:00+05:30",
            r#"[{"underlying":"SENSEX","expiry":"2026-10-29","strike":"74000","right":"CE","quantity":"-20"}]"#,
            SENSEX_OCTOBER_CALL_SHORT,
        ),
        (
            "short SENSEX 29 October 73000 put",
            "BSE",
            "2026-10-28T10:00:00+05:30",
            r#"[{"underlying":"SENSEX","expiry":"2026-10-29","strike":"73000","right":"PE","quantity":"-20"}]"#,
            SENSEX_OCTOBER_PUT_SHORT,
        ),
        (
            "long SENSEX 29 October 74000 call",
            "BSE",
            "2026-10-28T10:00:00+05:30",
            r#"[{"underlying":"SENSEX","expiry":"2026-10-29","strike":"74000","right":"CE","quantity":"20"}]"#,
            SENSEX_OCTOBER_CALL_LONG,
        ),
        (
            "short SENSEX November 74000 call",
            "BSE",
            "2026-10-28T10:00:00+05:30",
            r#"[{"underlying":"SENSEX","expiry":"2026-11-26","strike":"74000","right":"CE","quantity":"-20"}]"#,
            SENSEX_NOVEMBER_CALL_SHORT,
        ),
        (
            "short SENSEX November 75000 put",
            "BSE",
            "2026-10-28T10:00:00+05:30",
            r#"[{"underlying":"SENSEX","expiry":"2026-11-26","strike":"75000","right":"PE","quantity":"-20"}]"#,
            SENSEX_NOVEMBER_PUT_SHORT,
        ),
        (
            "long SENSEX November 75000 put",
            "BSE",
            "2026-10-28T10:00:00+05:30",
            r#"[{"underlying":"SENSEX","expiry":"2026-11-26","strike":"75000","right":"PE","quantity":"20"}]"#,
            SENSEX_NOVEMBER_PUT_LONG,
        ),
        (
            "HDFCBANK, TCS, and INFY futures and options",
            "NSE",
            TRADING_DATETIME,
            r#"[{"underlying":"HDFCBANK","expiry":"2026-11-23","right":"FUT","quantity":"-650"},{"underlying":"HDFCBANK","expiry":"2026-11-23","strike":"700","right":"CE","quantity":"-650"},{"underlying":"TCS","expiry":"2026-11-23","right":"FUT","quantity":"225"},{"underlying":"TCS","expiry":"2026-11-23","strike":"2440","right":"PE","quantity":"-225"},{"underlying":"INFY","expiry":"2026-11-23","right":"FUT","quantity":"-400"},{"underlying":"INFY","expiry":"2026-11-23","strike":"1100","right":"CE","quantity":"400"}]"#,
            STOCK_FUTURES_AND_OPTIONS,
        ),
    ];
    for (name, exchange, trading_datetime, positions, expected) in books {
        let body = format!(
            r#"{{"exchange":"{exchange}","trading_datetime":"{trading_datetime}","positions":{positions}}}"#
        );
        let (status, actual) = send(&state, &body).await;
        if status == StatusCode::NOT_FOUND && span_archive_missing(&actual) {
            eprintln!(
                "SPAN files for {trading_datetime} are not archived; skipping Angel One reply test"
            );
            return;
        }
        assert_eq!(status, StatusCode::OK, "{name}: {actual}");
        let expected: serde_json::Value = serde_json::from_str(expected).expect(name);
        assert_eq!(actual, expected, "{name}");
    }
}

fn span_archive_missing(body: &serde_json::Value) -> bool {
    body["error"]["code"] == "span_file_not_found"
        && body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("no official SPAN file is archived"))
}

async fn send(state: &AppState, body: &str) -> (StatusCode, serde_json::Value) {
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
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}
