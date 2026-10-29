//! Operator reconciliation against one archived SPAN snapshot.
//!
//! The reference supplies the expected component amounts. This module quotes
//! the same book on the SPAN file named by the reference and prints the
//! difference. It does not assign a diagnostic category, and a match does not
//! change the equivalence claim.

use chrono::{DateTime, FixedOffset, NaiveDateTime};
use margin_data::{
    AppConfig, AppStore, DataError, MarginPosition, canonical_contract_symbol,
    check_margin_positions, ensure_nse_margin_files,
};
use margin_domain::{ClearingCorporation, Currency, EQUIVALENCE_CLAIM, Exchange, Money};
use margin_reconciliation::{CompareInput, Tolerance, compare};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;

use crate::cli::EngineError;

#[derive(Debug)]
struct Reference {
    source: String,
    trading_datetime: NaiveDateTime,
    span_filename: Option<String>,
    span_sha256: Option<String>,
    positions: Vec<MarginPosition>,
    expected: ExpectedAmounts,
}

#[derive(Debug)]
struct ExpectedAmounts {
    span_margin: Decimal,
    extreme_loss_margin: Decimal,
    premium: Decimal,
    delivery_margin: Decimal,
    total_margin: Decimal,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceFile {
    source: String,
    exchange: String,
    trading_datetime: String,
    #[serde(default)]
    span_filename: Option<String>,
    #[serde(default)]
    span_sha256: Option<String>,
    positions: Vec<PositionFile>,
    expected: ExpectedFile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PositionFile {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    underlying: Option<String>,
    #[serde(default)]
    expiry: Option<String>,
    #[serde(default)]
    strike: Option<String>,
    #[serde(default, rename = "instrumentType")]
    instrument_type: Option<String>,
    quantity: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFile {
    span_margin: String,
    extreme_loss_margin: String,
    premium: String,
    delivery_margin: String,
    total_margin: String,
}

struct ComponentLine {
    name: &'static str,
    expected: Decimal,
    calculated: Decimal,
    absolute: Decimal,
    material: bool,
    within_tolerance: bool,
}

pub(crate) async fn reconcile(
    path: String,
    config: &AppConfig,
    store: &AppStore,
) -> Result<(), EngineError> {
    let reference = load_reference(&path)?;
    let stored = quote(config, store, &reference).await?;
    if let Some(filename) = &reference.span_filename
        && stored.filename != *filename
    {
        return Err(EngineError::Failed {
            detail: format!(
                "the SPAN file current at {} is {}, and the reference names {filename}",
                ist_stamp(reference.trading_datetime),
                stored.filename
            ),
        });
    }
    if let Some(sha256) = &reference.span_sha256
        && stored.sha256 != *sha256
    {
        return Err(EngineError::Failed {
            detail: format!(
                "the SPAN file current at {} has sha256 {}, and the reference names {sha256}",
                ist_stamp(reference.trading_datetime),
                stored.sha256
            ),
        });
    }
    let (report, material) = comparison_report(&reference, &stored)?;
    print!("{report}");
    if material {
        return Err(EngineError::Failed {
            detail: "reconciliation found a material difference".to_owned(),
        });
    }
    Ok(())
}

fn load_reference(path: &str) -> Result<Reference, EngineError> {
    let bytes = std::fs::read(path).map_err(|_| EngineError::InvalidArgument {
        detail: format!("reference `{path}` could not be read"),
    })?;
    parse_reference(&bytes).map_err(|detail| EngineError::InvalidArgument { detail })
}

async fn quote(
    config: &AppConfig,
    store: &AppStore,
    reference: &Reference,
) -> Result<margin_data::StoredSpanQuote, EngineError> {
    let exchange = Exchange::Nse;
    let attempt = store
        .span_margin(
            exchange,
            ClearingCorporation::Nscc,
            reference.trading_datetime,
            &reference.positions,
        )
        .await;
    let stored = match attempt {
        Err(error) if missing_archive(&error) => {
            if let AppStore::Postgres(postgres) = store {
                ensure_nse_margin_files(config, postgres, reference.trading_datetime.date())
                    .await
                    .map_err(quote_error)?;
            }
            store
                .span_margin(
                    exchange,
                    ClearingCorporation::Nscc,
                    reference.trading_datetime,
                    &reference.positions,
                )
                .await
                .map_err(quote_error)?
        }
        other => other.map_err(quote_error)?,
    };
    Ok(stored)
}

fn comparison_report(
    reference: &Reference,
    stored: &margin_data::StoredSpanQuote,
) -> Result<(String, bool), EngineError> {
    let quote = &stored.quote;
    let pairs = [
        (
            "span_margin",
            reference.expected.span_margin,
            quote.span_margin,
        ),
        (
            "extreme_loss_margin",
            reference.expected.extreme_loss_margin,
            quote.extreme_loss_margin,
        ),
        ("premium", reference.expected.premium, quote.premium),
        (
            "delivery_margin",
            reference.expected.delivery_margin,
            quote.delivery_margin,
        ),
        (
            "total_margin",
            reference.expected.total_margin,
            quote.total_margin,
        ),
    ];
    let lines = compare_components(&pairs)?;
    let material = lines.iter().any(|line| line.material);
    let mut report = format!(
        "equivalence_claim={EQUIVALENCE_CLAIM}\nsource={}\ntrading_datetime={}\nspan_filename={}\nspan_sha256={}\nspan_created={}\ndataset_id={}\n",
        reference.source,
        ist_stamp(reference.trading_datetime),
        stored.filename,
        stored.sha256,
        ist_stamp(stored.span_created),
        stored.dataset_id
    );
    for line in lines {
        report.push_str(&format!(
            "component={} expected={} calculated={} absolute={} material={} within_tolerance={}\n",
            line.name,
            amount_text(line.expected),
            amount_text(line.calculated),
            amount_text(line.absolute),
            line.material,
            line.within_tolerance
        ));
    }
    Ok((report, material))
}

fn compare_components(
    pairs: &[(&'static str, Decimal, Decimal)],
) -> Result<Vec<ComponentLine>, EngineError> {
    let mut lines = Vec::with_capacity(pairs.len());
    for (name, expected, calculated) in pairs {
        let difference = compare(CompareInput {
            expected: Money::from_decimal(*expected, Currency::Inr),
            calculated: Money::from_decimal(*calculated, Currency::Inr),
            tolerance: Tolerance::new(Money::zero(Currency::Inr), Decimal::ZERO).map_err(
                |error| EngineError::Failed {
                    detail: error.to_string(),
                },
            )?,
            category: None,
            position_refs: Vec::new(),
            risk_dataset_id: None,
            calculation_id: None,
            rule_version_id: None,
            calculation_trace: format!("operator reference component {name}"),
        })
        .map_err(|error| EngineError::Failed {
            detail: error.to_string(),
        })?;
        lines.push(ComponentLine {
            name,
            expected: *expected,
            calculated: *calculated,
            absolute: difference.absolute().amount(),
            material: difference.material(),
            within_tolerance: difference.within_diagnostic_tolerance(),
        });
    }
    Ok(lines)
}

fn parse_reference(bytes: &[u8]) -> Result<Reference, String> {
    let file: ReferenceFile = serde_json::from_slice(bytes)
        .map_err(|error| format!("reference JSON could not be read: {error}"))?;
    let exchange = Exchange::parse(file.exchange.trim()).map_err(|error| error.to_string())?;
    if exchange != Exchange::Nse {
        return Err("SPAN reconciliation is available for archived NSE files".to_owned());
    }
    let source = file.source.trim();
    if source.is_empty() || source.len() > 200 || source.chars().any(char::is_control) {
        return Err("source must be 1 to 200 characters without control characters".to_owned());
    }
    let trading_datetime = parse_trading_datetime(&file.trading_datetime)?;
    let span_filename = filled(file.span_filename);
    let span_sha256 = filled(file.span_sha256);
    if span_filename.is_none() && span_sha256.is_none() {
        return Err("the reference must name span_filename or span_sha256".to_owned());
    }
    if let Some(filename) = &span_filename
        && (filename.contains('/') || filename.contains('\\'))
    {
        return Err("span_filename must be the archived filename".to_owned());
    }
    let span_sha256 = match span_sha256 {
        Some(sha256) => Some(
            margin_domain::Sha256Hex::parse(&sha256)
                .map_err(|error| error.to_string())?
                .as_str()
                .to_owned(),
        ),
        None => None,
    };
    if file.positions.is_empty() {
        return Err("at least one position is required".to_owned());
    }
    let mut positions = Vec::with_capacity(file.positions.len());
    for position in &file.positions {
        let symbol = position_symbol(exchange, position)?;
        let quantity = Decimal::from_str(position.quantity.trim())
            .map_err(|_| format!("quantity for {symbol} must be a decimal string"))?;
        if quantity == Decimal::ZERO {
            return Err(format!("quantity for {symbol} is zero"));
        }
        positions.push(MarginPosition { symbol, quantity });
    }
    check_margin_positions(&positions)?;
    Ok(Reference {
        source: source.to_owned(),
        trading_datetime,
        span_filename,
        span_sha256,
        positions,
        expected: ExpectedAmounts {
            span_margin: amount("span_margin", &file.expected.span_margin)?,
            extreme_loss_margin: amount("extreme_loss_margin", &file.expected.extreme_loss_margin)?,
            premium: amount("premium", &file.expected.premium)?,
            delivery_margin: amount("delivery_margin", &file.expected.delivery_margin)?,
            total_margin: amount("total_margin", &file.expected.total_margin)?,
        },
    })
}

fn position_symbol(exchange: Exchange, position: &PositionFile) -> Result<String, String> {
    let underlying = filled_ref(position.underlying.as_deref());
    let expiry = filled_ref(position.expiry.as_deref());
    let strike = filled_ref(position.strike.as_deref());
    let right = filled_ref(position.instrument_type.as_deref());
    let structured =
        underlying.is_some() || expiry.is_some() || strike.is_some() || right.is_some();
    let symbol = filled_ref(position.symbol.as_deref());
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

fn amount(name: &str, value: &str) -> Result<Decimal, String> {
    Decimal::from_str(value.trim()).map_err(|_| format!("{name} must be a decimal string"))
}

fn filled(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn filled_ref(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
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

fn ist_stamp(value: NaiveDateTime) -> String {
    format!("{}+05:30", value.format("%Y-%m-%dT%H:%M:%S"))
}

fn amount_text(amount: Decimal) -> String {
    amount.normalize().to_string()
}

fn missing_archive(error: &DataError) -> bool {
    match error {
        DataError::MarginNotFound { .. } => true,
        DataError::MarginRejected { detail } => {
            detail.contains("no exposure file is archived")
                || detail.contains("no contract ELM file is archived")
                || detail.contains("capital-market VaR rate is not in the archived files")
                || detail.contains("no contract master is archived")
        }
        _ => false,
    }
}

fn quote_error(error: DataError) -> EngineError {
    EngineError::Failed {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_components, parse_reference};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn decimal(value: &str) -> Decimal {
        Decimal::from_str(value).expect("decimal")
    }

    fn reference(extra: &str) -> String {
        format!(
            r#"{{
                "source": "operator copy",
                "exchange": "NSE",
                "trading_datetime": "2026-10-28T14:30:00+05:30",
                "span_filename": "nsccl.20261028.i4.zip",
                {extra}
                "positions": [{{"underlying": "NIFTY", "expiry": "2026-11-23", "instrumentType": "FUT", "quantity": "-65"}}],
                "expected": {{
                    "span_margin": "1.00",
                    "extreme_loss_margin": "2",
                    "premium": "0",
                    "delivery_margin": "0.00",
                    "total_margin": "3"
                }}
            }}"#
        )
    }

    #[test]
    fn a_reference_names_the_span_file_and_the_component_amounts() {
        let parsed = parse_reference(reference("").as_bytes()).expect("reference");
        assert_eq!(
            parsed.span_filename.as_deref(),
            Some("nsccl.20261028.i4.zip")
        );
        assert_eq!(parsed.positions[0].symbol, "NSE:NIFTY:2026-11-23:FUT");
        assert_eq!(parsed.expected.total_margin.to_string(), "3");
        assert!(parse_reference(reference("\"span_sha256\": \"abcd\",").as_bytes()).is_err());
        let mut without_file = reference("");
        without_file = without_file.replace("\"span_filename\": \"nsccl.20261028.i4.zip\",", "");
        assert!(
            parse_reference(without_file.as_bytes())
                .expect_err("identity")
                .contains("span_filename")
        );
    }

    #[test]
    fn a_difference_stays_material_and_has_no_category() {
        let matched = compare_components(&[
            ("span_margin", decimal("1"), decimal("1.00")),
            ("extreme_loss_margin", decimal("2"), decimal("2")),
            ("premium", decimal("0"), decimal("0")),
            ("delivery_margin", decimal("0"), decimal("0")),
            ("total_margin", decimal("3"), decimal("3")),
        ])
        .expect("matched");
        assert!(matched.iter().all(|line| !line.material));
        let differed = compare_components(&[("total_margin", decimal("3"), decimal("3.01"))])
            .expect("differed");
        assert!(differed[0].material);
        assert!(!differed[0].within_tolerance);
        assert_eq!(differed[0].absolute.to_string(), "0.01");
    }
}
