//! Contracts selected from a SPAN file, and the symbol that names each one.

use super::symbol::{MarginPosition, SymbolSpec, expiry_matches, week_code};
use crate::zip_safe::{ZipLimits, extract_zip_to_memory};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const UNCOMPRESSED_LIMIT: u64 = 104_857_600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ContractKind {
    Future,
    Call,
    Put,
    Stock,
}

impl ContractKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Future => "future",
            Self::Call => "call",
            Self::Put => "put",
            Self::Stock => "stock",
        }
    }

    pub(super) fn is_option(self) -> bool {
        matches!(self, Self::Call | Self::Put)
    }
}

pub(super) fn span_xml_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut entries = extract_zip_to_memory(bytes, ZipLimits::conservative(UNCOMPRESSED_LIMIT))
        .map_err(|error| error.to_string())?;
    let member = entries.first().map(|entry| entry.name.to_ascii_lowercase());
    let span_member = member
        .as_deref()
        .is_some_and(|name| name.ends_with(".spn") || name.ends_with(".xml"));
    if entries.len() != 1 || !span_member {
        return Err("archived SPAN zip does not contain one .spn or .xml member".to_owned());
    }
    entries
        .pop()
        .map(|entry| entry.bytes)
        .ok_or_else(|| "archived SPAN zip does not contain one .spn or .xml member".to_owned())
}

pub(super) fn resolve_positions<'a>(
    positions: &[MarginPosition],
    specs: &[SymbolSpec],
    book: &'a Book,
) -> Result<Vec<Requested<'a>>, String> {
    let mut requested = Vec::with_capacity(positions.len());
    let mut seen = BTreeSet::new();
    for (position, spec) in positions.iter().zip(specs.iter()) {
        let contract = select_contract(&position.symbol, spec, &book.contracts)?;
        let key = (
            contract.kind,
            contract.product.clone(),
            contract.expiry,
            contract.strike,
        );
        if !seen.insert(key) {
            return Err(format!(
                "{} selects the same contract as another position",
                position.symbol
            ));
        }
        requested.push(Requested {
            symbol: position.symbol.clone(),
            quantity: position.quantity,
            contract,
        });
    }
    Ok(requested)
}

pub(super) struct Requested<'a> {
    pub(super) symbol: String,
    pub(super) quantity: Decimal,
    pub(super) contract: &'a SpanContract,
}

#[derive(Clone)]
pub(crate) struct SpanContract {
    pub kind: ContractKind,
    pub product: String,
    pub expiry: Option<NaiveDate>,
    pub strike: Option<Decimal>,
    pub settlement_price: Decimal,
    pub delta: Option<Decimal>,
    pub cvf: Decimal,
    pub scaling: Decimal,
    pub values: Vec<Decimal>,
}

#[derive(Clone)]
pub(crate) struct Commodity {
    pub risk_exponent: i32,
    pub combination_method: String,
    pub som_is_zero: bool,
}

pub(super) struct Book {
    pub(super) exchange: String,
    pub(super) contracts: Vec<SpanContract>,
    pub(super) commodities: BTreeMap<String, Commodity>,
    pub(super) underlyings: BTreeMap<String, Decimal>,
    pub(super) spreads: BTreeMap<String, Vec<CalendarSpread>>,
    pub(super) indexes: BTreeSet<String>,
}

#[derive(Clone)]
pub(crate) struct CalendarSpread {
    pub number: u32,
    pub charge: Decimal,
    pub method: String,
    pub legs: [(NaiveDate, Decimal); 2],
}

fn select_contract<'a>(
    symbol: &str,
    spec: &SymbolSpec,
    contracts: &'a [SpanContract],
) -> Result<&'a SpanContract, String> {
    match spec {
        SymbolSpec::Stock { product } => {
            let matches = contracts
                .iter()
                .filter(|contract| {
                    contract.kind == ContractKind::Stock && contract.product == *product
                })
                .collect::<Vec<_>>();
            one_contract(
                symbol,
                matches,
                format!("{symbol} is not a physical stock in the SPAN file"),
                format!("{symbol} matches more than one physical contract"),
            )
        }
        SymbolSpec::Future {
            product,
            year,
            month,
            day,
            instrument: _,
        } => {
            let matches = contracts
                .iter()
                .filter(|contract| {
                    contract.kind == ContractKind::Future
                        && contract.product == *product
                        && expiry_matches(contract.expiry, *year, *month, *day)
                })
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                return Err(ambiguous_futures(symbol, &matches));
            }
            one_contract(
                symbol,
                matches,
                format!("{symbol} is not a futures contract in the SPAN file"),
                format!("{symbol} matches more than one futures expiry"),
            )
        }
        SymbolSpec::Option { .. } => select_option(symbol, spec, contracts),
    }
}

fn select_option<'a>(
    symbol: &str,
    spec: &SymbolSpec,
    contracts: &'a [SpanContract],
) -> Result<&'a SpanContract, String> {
    let SymbolSpec::Option {
        product,
        year,
        month,
        day,
        strike,
        call,
        instrument: _,
    } = spec
    else {
        return Err(format!("{symbol} is not an option in the SPAN file"));
    };
    let kind = if *call {
        ContractKind::Call
    } else {
        ContractKind::Put
    };
    let matches = contracts
        .iter()
        .filter(|contract| {
            contract.kind == kind
                && contract.product == *product
                && contract.strike == Some(*strike)
                && expiry_matches(contract.expiry, *year, *month, *day)
        })
        .collect::<Vec<_>>();
    if day.is_none() && matches.len() > 1 {
        let anchors = contracts
            .iter()
            .filter(|contract| {
                contract.kind == ContractKind::Future
                    && contract.product == *product
                    && expiry_matches(contract.expiry, *year, *month, None)
            })
            .filter_map(|contract| contract.expiry)
            .collect::<BTreeSet<_>>();
        if anchors.len() == 1 {
            let Some(expiry) = anchors.iter().copied().next() else {
                return Err(ambiguous_options(symbol, &matches));
            };
            let anchored = matches
                .iter()
                .copied()
                .filter(|contract| contract.expiry == Some(expiry))
                .collect::<Vec<_>>();
            if anchored.len() == 1 {
                return Ok(anchored[0]);
            }
            if anchored.is_empty() {
                return Err(format!(
                    "{symbol} does not exist on the futures expiry {expiry}. Dated symbols: {}",
                    option_choices(&matches)
                ));
            }
            return Err(ambiguous_options(symbol, &anchored));
        }
        return Err(ambiguous_options(symbol, &matches));
    }
    one_contract(
        symbol,
        matches,
        format!("{symbol} is not an option in the SPAN file"),
        format!("{symbol} matches more than one option expiry"),
    )
}

fn one_contract<'a>(
    _symbol: &str,
    matches: Vec<&'a SpanContract>,
    missing: String,
    ambiguous: String,
) -> Result<&'a SpanContract, String> {
    match matches.as_slice() {
        [contract] => Ok(*contract),
        [] => Err(missing),
        _ => Err(ambiguous),
    }
}

fn ambiguous_futures(symbol: &str, matches: &[&SpanContract]) -> String {
    format!(
        "{symbol} matches more than one futures expiry: {}",
        future_choices(matches)
    )
}

fn ambiguous_options(symbol: &str, matches: &[&SpanContract]) -> String {
    format!(
        "{symbol} matches more than one option expiry: {}",
        option_choices(matches)
    )
}

fn future_choices(matches: &[&SpanContract]) -> String {
    matches
        .iter()
        .filter_map(|contract| {
            let expiry = contract.expiry?;
            Some(format!(
                "{} ({expiry})",
                dated_future(&contract.product, expiry)
            ))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn option_choices(matches: &[&SpanContract]) -> String {
    matches
        .iter()
        .filter_map(|contract| {
            let expiry = contract.expiry?;
            let strike = contract.strike?;
            Some(format!(
                "{} ({expiry})",
                dated_option(
                    &contract.product,
                    expiry,
                    strike,
                    contract.kind == ContractKind::Call
                )
            ))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn canonical_symbol(exchange: &str, contract: &SpanContract) -> String {
    let Some(expiry) = contract.expiry else {
        return format!("{exchange}:{}:EQ", contract.product);
    };
    canonical_parts(
        exchange,
        &contract.product,
        expiry,
        contract.strike,
        contract.kind,
    )
}

pub(super) fn canonical_parts(
    exchange: &str,
    product: &str,
    expiry: NaiveDate,
    strike: Option<Decimal>,
    kind: ContractKind,
) -> String {
    match kind {
        ContractKind::Stock => format!("{exchange}:{product}:EQ"),
        ContractKind::Future => format!("{exchange}:{product}:{expiry}:FUT"),
        ContractKind::Call => format!(
            "{exchange}:{product}:{expiry}:{}:CE",
            strike.unwrap_or(Decimal::ZERO).normalize()
        ),
        ContractKind::Put => format!(
            "{exchange}:{product}:{expiry}:{}:PE",
            strike.unwrap_or(Decimal::ZERO).normalize()
        ),
    }
}

fn dated_future(product: &str, expiry: NaiveDate) -> String {
    format!(
        "{product}{:02}{}{:02}FUT",
        expiry.year() % 100,
        week_code(expiry.month()),
        expiry.day()
    )
}

fn dated_option(product: &str, expiry: NaiveDate, strike: Decimal, call: bool) -> String {
    let right = if call { "CE" } else { "PE" };
    format!(
        "{product}{:02}{}{:02}{}{right}",
        expiry.year() % 100,
        week_code(expiry.month()),
        expiry.day(),
        strike.normalize()
    )
}
