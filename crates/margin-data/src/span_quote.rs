//! SPAN scanning risk, extreme loss margin, and option premium.
//!
//! NSE Clearing states that a risk-array loss is positive and a gain is negative
//! for a long unit. Scanning risk is the greatest scenario loss of the book.
//! Monthly symbols use `YY` and `MON`. Dated symbols use `YY`, the month code
//! `1`–`9`, `O`, `N`, or `D`, and `DD`. A physical stock is the product code.
//! Stock membership comes from the exposure file. Extreme-loss percents come
//! from the contract-wise ELM file when that file is loaded.

use crate::zip_safe::{ZipLimits, extract_zip_to_memory};
use chrono::{Datelike, NaiveDate, Weekday};
use flate2::read::GzDecoder;
use quick_xml::Reader;
use quick_xml::events::Event;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::str::FromStr;

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const UNCOMPRESSED_LIMIT: u64 = 104_857_600;
const NOTE: &str = "SPAN scanning risk from the archived XML risk arrays. Loss is positive for a long unit. Scanning risk is the greatest scenario loss, and it is zero when every scenario is a gain. Calendar spreads add the flat charge from the SPAN file and, for futures, extreme loss margin on one third of the far-month quantity. An index spread that includes a contract expiring on the trade date is not offset. Reported amounts are rounded half away from zero to paise. Delivery margin on a stock derivative is 10, 25, 45, or 70 percent of the archived cash-market daily margin rate from four trading days before expiry through the day before expiry, and the full rate on the expiry day. The quantity is the net shares that expiry would deliver. It is zero outside that window and for an index. The total is SPAN margin plus exposure margin plus delivery margin. Margin benefit compares the separate position totals with the total before delivery margin. Cross-margin benefit is zero.";
const NOTE_SHORT_OPTION: &str = "SPAN scanning risk from the archived XML risk arrays. Loss is positive for a long unit. Scanning risk is the greatest scenario loss, and it is zero when every scenario is a gain. The short-option minimum rate in the SPAN file is zero. A short option's extreme loss margin uses the contract file's percent when that file is loaded, and otherwise the published index or exposure-file percent. Notional is the SPAN underlying price. Delivery margin on a stock derivative is 10, 25, 45, or 70 percent of the archived cash-market daily margin rate from four trading days before expiry through the day before expiry, and the full rate on the expiry day. The quantity is the net shares that expiry would deliver. The total is SPAN margin plus exposure margin plus delivery margin, and the short option premium is included in that total. A book of only long options has a total equal to its delivery margin. Margin benefit compares the separate position totals with the total before delivery margin. Cross-margin benefit is zero.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarginPosition {
    pub symbol: String,
    pub quantity: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarginLine {
    pub symbol: String,
    pub canonical: String,
    pub instrument: &'static str,
    pub product: String,
    pub expiry: Option<NaiveDate>,
    pub strike: Option<Decimal>,
    pub quantity: Decimal,
    pub settlement_price: Decimal,
    pub underlying_price: Option<Decimal>,
    pub scanning_risk: Decimal,
    pub extreme_loss_margin: Decimal,
    pub elm_percent: Decimal,
    pub premium: Decimal,
    pub total_margin: Decimal,
    pub lot_size: Option<u32>,
    pub worst_scan_points: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanMarginQuote {
    pub currency: &'static str,
    pub scanning_risk: Decimal,
    pub spread_charge: Decimal,
    pub short_option_minimum: Decimal,
    pub span_margin: Decimal,
    pub extreme_loss_margin: Decimal,
    pub premium: Decimal,
    pub premium_margin: Decimal,
    pub delivery_margin: Decimal,
    pub cross_margin_benefit: Decimal,
    pub margin_benefit: Decimal,
    pub total_margin: Decimal,
    pub lines: Vec<MarginLine>,
    pub note: &'static str,
}

impl SpanMarginQuote {
    pub fn apply_lot_sizes(&mut self, master: &crate::ContractMaster) -> Result<(), String> {
        for line in &mut self.lines {
            line.lot_size =
                master.lot_for(line.instrument, &line.product, line.expiry, line.strike)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ContractKind {
    Future,
    Call,
    Put,
    Stock,
}

impl ContractKind {
    fn label(self) -> &'static str {
        match self {
            Self::Future => "future",
            Self::Call => "call",
            Self::Put => "put",
            Self::Stock => "stock",
        }
    }

    fn is_option(self) -> bool {
        matches!(self, Self::Call | Self::Put)
    }
}

pub fn quote_span_zip(
    bytes: &[u8],
    trading_date: NaiveDate,
    positions: &[MarginPosition],
    exposure: &ExposureRates,
    contracts: &ContractElmRates,
    cash: &CashMarginRates,
) -> Result<SpanMarginQuote, String> {
    let entries = extract_zip_to_memory(bytes, ZipLimits::conservative(UNCOMPRESSED_LIMIT))
        .map_err(|error| error.to_string())?;
    if entries.len() != 1 || !entries[0].name.ends_with(".spn") {
        return Err("archived SPAN zip does not contain one .spn member".to_owned());
    }
    quote_span_xml(
        &entries[0].bytes,
        trading_date,
        positions,
        exposure,
        contracts,
        cash,
    )
}

pub fn quote_span_xml(
    bytes: &[u8],
    trading_date: NaiveDate,
    positions: &[MarginPosition],
    exposure: &ExposureRates,
    contracts: &ContractElmRates,
    cash: &CashMarginRates,
) -> Result<SpanMarginQuote, String> {
    let specs = parse_positions(positions)?;
    let specs = contracts.identify(positions, specs)?;
    let book = extract_contracts(bytes, trading_date, &specs)?;
    let requested = resolve_positions(positions, &specs, &book)?;
    price_book(
        positions,
        &requested,
        &book,
        trading_date,
        exposure,
        contracts,
        cash,
    )
}

pub fn check_margin_positions(positions: &[MarginPosition]) -> Result<(), String> {
    parse_positions(positions).map(|_| ())
}

pub fn canonical_contract_symbol(
    exchange_code: &str,
    underlying: &str,
    expiry: Option<&str>,
    strike_text: Option<&str>,
    right: &str,
) -> Result<String, String> {
    if exchange_code != "NSE" {
        return Err("SPAN margin symbols use the NSE exchange".to_owned());
    }
    let product = underlying.trim().to_ascii_uppercase();
    if !ascii_product(&product) {
        return Err("underlying must be the product code".to_owned());
    }
    let right = right.trim().to_ascii_uppercase();
    match right.as_str() {
        "FUT" => {
            if strike_text.is_some_and(|strike| !strike.trim().is_empty()) {
                return Err("a future has no strike".to_owned());
            }
            let expiry = require_expiry(expiry)?;
            Ok(format!("NSE:{product}:{expiry}:FUT"))
        }
        "EQ" => {
            if expiry.is_some_and(|expiry| !expiry.trim().is_empty())
                || strike_text.is_some_and(|strike| !strike.trim().is_empty())
            {
                return Err("a physical stock has no expiry or strike".to_owned());
            }
            Ok(format!("NSE:{product}:EQ"))
        }
        "C" | "CE" | "P" | "PE" => {
            let expiry = require_expiry(expiry)?;
            let strike = strike_text
                .map(str::trim)
                .filter(|strike| !strike.is_empty())
                .ok_or_else(|| "an option needs a strike".to_owned())?;
            let strike = Decimal::from_str(strike)
                .map_err(|_| "strike must be a decimal string".to_owned())?;
            if strike <= Decimal::ZERO {
                return Err("strike must be positive".to_owned());
            }
            let side = if right.starts_with('C') { "CE" } else { "PE" };
            Ok(format!(
                "NSE:{product}:{expiry}:{}:{side}",
                strike.normalize()
            ))
        }
        _ => Err("right must be FUT, C, P, CE, PE, or EQ".to_owned()),
    }
}

fn require_expiry(expiry: Option<&str>) -> Result<NaiveDate, String> {
    let expiry = expiry
        .map(str::trim)
        .filter(|expiry| !expiry.is_empty())
        .ok_or_else(|| "expiry must be YYYY-MM-DD".to_owned())?;
    NaiveDate::parse_from_str(expiry, "%Y-%m-%d")
        .map_err(|_| "expiry must be YYYY-MM-DD".to_owned())
}

fn parse_positions(positions: &[MarginPosition]) -> Result<Vec<SymbolSpec>, String> {
    if positions.is_empty() {
        return Err("at least one position is required".to_owned());
    }
    if positions.len() > 32 {
        return Err("a margin quote accepts at most 32 positions".to_owned());
    }
    let mut seen_symbols = BTreeSet::new();
    let mut specs = Vec::with_capacity(positions.len());
    for position in positions {
        if !seen_symbols.insert(position.symbol.to_ascii_uppercase()) {
            return Err(format!("{} is repeated", position.symbol));
        }
        if position.quantity == Decimal::ZERO {
            return Err(format!("quantity for {} is zero", position.symbol));
        }
        specs.push(parse_symbol(&position.symbol)?);
    }
    Ok(specs)
}

fn resolve_positions<'a>(
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

fn price_book(
    positions: &[MarginPosition],
    requested: &[Requested<'_>],
    book: &Book,
    trading_date: NaiveDate,
    exposure: &ExposureRates,
    contracts: &ContractElmRates,
    cash: &CashMarginRates,
) -> Result<SpanMarginQuote, String> {
    let mut by_product: BTreeMap<&str, Vec<&Requested<'_>>> = BTreeMap::new();
    for position in requested {
        by_product
            .entry(position.contract.product.as_str())
            .or_default()
            .push(position);
    }
    let mut scanning_risk = Decimal::ZERO;
    let mut spread_charge = Decimal::ZERO;
    let mut extreme_loss_margin = Decimal::ZERO;
    let mut premium = Decimal::ZERO;
    let mut delivery_margin = Decimal::ZERO;
    let mut lines = Vec::new();
    let mut short_option = false;
    for (product, group) in by_product {
        let commodity = book
            .commodities
            .get(product)
            .ok_or_else(|| format!("SPAN combined commodity {product} is missing"))?;
        if commodity.risk_exponent != 0 {
            return Err(format!(
                "SPAN riskExponent {} for {product} is not applied to XML risk arrays",
                commodity.risk_exponent
            ));
        }
        if commodity.combination_method != "NONE" {
            return Err(format!(
                "SPAN combination method {} for {product} is not applied",
                commodity.combination_method
            ));
        }
        let derivative_expiries: BTreeSet<NaiveDate> = group
            .iter()
            .filter_map(|position| position.contract.expiry)
            .collect();
        let index = !exposure.contains_stock(product);
        let separate_expiring = index && derivative_expiries.contains(&trading_date);
        let width = group[0].contract.values.len();
        if width == 0
            || group
                .iter()
                .any(|position| position.contract.values.len() != width)
        {
            return Err(format!(
                "{product} risk arrays do not share one scan-point count"
            ));
        }
        for position in &group {
            if position.contract.kind.is_option() && position.quantity.is_sign_negative() {
                short_option = true;
                if !commodity.som_is_zero {
                    return Err(format!(
                        "SPAN short-option minimum for {product} is not a single zero rate, so it is not applied"
                    ));
                }
            }
            if position.contract.cvf != Decimal::ONE || position.contract.scaling != Decimal::ONE {
                return Err(format!(
                    "{} contract value factor {} and scaling {} are not both 1, so the XML risk array is not scaled",
                    position.symbol, position.contract.cvf, position.contract.scaling
                ));
            }
        }
        let commodity_risk = if separate_expiring {
            let (today_risk, _) = scan_positions(
                group
                    .iter()
                    .copied()
                    .filter(|position| position.contract.expiry == Some(trading_date)),
                width,
                product,
            )?;
            let (rest_risk, _) = scan_positions(
                group
                    .iter()
                    .copied()
                    .filter(|position| position.contract.expiry != Some(trading_date)),
                width,
                product,
            )?;
            today_risk
                .checked_add(rest_risk)
                .ok_or_else(|| format!("SPAN scanning risk for {product} overflowed"))?
        } else {
            let (risk, _) = scan_positions(group.iter().copied(), width, product)?;
            risk
        };
        scanning_risk = scanning_risk
            .checked_add(commodity_risk)
            .ok_or_else(|| "SPAN scanning risk overflowed".to_owned())?;
        let formed = if derivative_expiries.len() > 1 {
            form_calendar_spreads(
                product,
                &group,
                book.spreads.get(product).map(Vec::as_slice).unwrap_or(&[]),
                trading_date,
                index,
            )?
        } else {
            FormedSpreads::default()
        };
        spread_charge = spread_charge
            .checked_add(formed.charge)
            .ok_or_else(|| format!("SPAN spread charge for {product} overflowed"))?;
        for position in &group {
            let own: Vec<Decimal> = position
                .contract
                .values
                .iter()
                .map(|value| position.quantity * *value)
                .collect();
            let (own_risk, own_points) = greatest_loss(&own)?;
            let charge = if position.contract.kind == ContractKind::Future {
                future_extreme_loss(product, position, &formed, exposure, contracts)?
            } else {
                extreme_loss(ExtremeLossInput {
                    product,
                    kind: position.contract.kind,
                    quantity: position.quantity,
                    settlement_price: position.contract.settlement_price,
                    strike: position.contract.strike,
                    expiry: position.contract.expiry,
                    underlying: book.underlyings.get(product).copied(),
                    trading_date,
                    exposure,
                    contracts,
                })?
            };
            let line_premium = option_premium(
                position.contract.kind,
                position.quantity,
                position.contract.settlement_price,
            )?;
            extreme_loss_margin = extreme_loss_margin
                .checked_add(charge.amount)
                .ok_or_else(|| format!("extreme loss margin for {product} overflowed"))?;
            premium = premium
                .checked_add(line_premium)
                .ok_or_else(|| format!("option premium for {product} overflowed"))?;
            let standalone_elm = if position.contract.kind == ContractKind::Future {
                apply_percent(
                    position.contract.settlement_price,
                    position.quantity.abs(),
                    charge.percent,
                )?
            } else {
                charge.amount
            };
            let short_premium = if line_premium.is_sign_negative() {
                Decimal::ZERO
                    .checked_sub(line_premium)
                    .ok_or_else(|| format!("option premium for {} overflowed", position.symbol))?
            } else {
                Decimal::ZERO
            };
            let standalone = if position.contract.kind.is_option()
                && !position.quantity.is_sign_negative()
            {
                Decimal::ZERO
            } else {
                own_risk
                    .checked_add(standalone_elm)
                    .and_then(|amount| amount.checked_add(short_premium))
                    .ok_or_else(|| format!("position total for {} overflowed", position.symbol))?
            };
            lines.push(MarginLine {
                symbol: position.symbol.clone(),
                canonical: canonical_symbol(position.contract),
                instrument: position.contract.kind.label(),
                product: product.to_owned(),
                expiry: position.contract.expiry,
                strike: position.contract.strike,
                quantity: position.quantity,
                settlement_price: position.contract.settlement_price,
                underlying_price: charge.underlying,
                scanning_risk: own_risk,
                extreme_loss_margin: charge.amount,
                elm_percent: charge.percent,
                premium: line_premium,
                total_margin: standalone,
                lot_size: None,
                worst_scan_points: own_points,
            });
        }
        let product_delivery = product_delivery(
            product,
            &group,
            book.underlyings.get(product).copied(),
            trading_date,
            index,
            cash,
        )?;
        delivery_margin = delivery_margin
            .checked_add(product_delivery)
            .ok_or_else(|| format!("delivery margin for {product} overflowed"))?;
    }
    let premium_margin = if premium.is_sign_negative() {
        Decimal::ZERO
    } else {
        premium
    };
    let cross_margin_benefit = Decimal::ZERO;
    let long_options_only = !lines.is_empty()
        && lines.iter().all(|line| {
            matches!(line.instrument, "call" | "put") && !line.quantity.is_sign_negative()
        });
    let scanning_risk = paise(scanning_risk);
    let spread_charge = paise(spread_charge);
    let short_option_minimum = Decimal::ZERO;
    let extreme_loss_margin = paise(extreme_loss_margin);
    let premium = paise(premium);
    let premium_margin = paise(premium_margin);
    let delivery_margin = paise(delivery_margin);
    let short_premium = if premium.is_sign_negative() {
        Decimal::ZERO
            .checked_sub(premium)
            .ok_or_else(|| "option premium overflowed".to_owned())?
    } else {
        Decimal::ZERO
    };
    let span_margin = scanning_risk
        .checked_add(spread_charge)
        .and_then(|amount| amount.checked_add(short_option_minimum))
        .ok_or_else(|| "SPAN margin overflowed".to_owned())?;
    let total_margin = if long_options_only {
        delivery_margin
    } else {
        span_margin
            .checked_add(extreme_loss_margin)
            .and_then(|amount| amount.checked_add(short_premium))
            .and_then(|amount| amount.checked_add(delivery_margin))
            .and_then(|amount| amount.checked_sub(cross_margin_benefit))
            .ok_or_else(|| "total margin overflowed".to_owned())?
    };
    let ordered: Vec<MarginLine> = positions
        .iter()
        .filter_map(|position| {
            lines
                .iter()
                .find(|line| line.symbol == position.symbol)
                .cloned()
        })
        .map(|mut line| {
            line.scanning_risk = paise(line.scanning_risk);
            line.extreme_loss_margin = paise(line.extreme_loss_margin);
            line.premium = paise(line.premium);
            line.elm_percent = paise(line.elm_percent);
            line.total_margin = if long_options_only {
                Decimal::ZERO
            } else {
                paise(line.total_margin)
            };
            line
        })
        .collect();
    let gross_margin = ordered
        .iter()
        .try_fold(Decimal::ZERO, |total, line| {
            total.checked_add(line.total_margin)
        })
        .ok_or_else(|| "position total overflowed".to_owned())?;
    let before_delivery = total_margin
        .checked_sub(delivery_margin)
        .ok_or_else(|| "total margin overflowed".to_owned())?;
    let margin_benefit = gross_margin
        .checked_sub(before_delivery)
        .filter(|gap| gap.is_sign_positive())
        .unwrap_or(Decimal::ZERO);
    Ok(SpanMarginQuote {
        currency: "INR",
        scanning_risk,
        spread_charge,
        short_option_minimum,
        span_margin,
        extreme_loss_margin,
        premium,
        premium_margin,
        delivery_margin,
        cross_margin_benefit,
        margin_benefit,
        total_margin,
        lines: ordered,
        note: if short_option {
            NOTE_SHORT_OPTION
        } else {
            NOTE
        },
    })
}

struct Requested<'a> {
    symbol: String,
    quantity: Decimal,
    contract: &'a SpanContract,
}

struct SpanContract {
    kind: ContractKind,
    product: String,
    expiry: Option<NaiveDate>,
    strike: Option<Decimal>,
    settlement_price: Decimal,
    delta: Option<Decimal>,
    cvf: Decimal,
    scaling: Decimal,
    values: Vec<Decimal>,
}

struct Commodity {
    risk_exponent: i32,
    combination_method: String,
    som_is_zero: bool,
}

struct Book {
    contracts: Vec<SpanContract>,
    commodities: BTreeMap<String, Commodity>,
    underlyings: BTreeMap<String, Decimal>,
    spreads: BTreeMap<String, Vec<CalendarSpread>>,
}

struct CalendarSpread {
    number: u32,
    charge: Decimal,
    method: String,
    legs: [(NaiveDate, Decimal); 2],
}

fn greatest_loss(losses: &[Decimal]) -> Result<(Decimal, Vec<u32>), String> {
    let mut worst: Option<Decimal> = None;
    let mut points = Vec::new();
    for (index, loss) in losses.iter().enumerate() {
        let point = u32::try_from(index + 1).unwrap_or(u32::MAX);
        match worst {
            None => {
                worst = Some(*loss);
                points.push(point);
            }
            Some(current) if *loss > current => {
                worst = Some(*loss);
                points.clear();
                points.push(point);
            }
            Some(current) if *loss == current => points.push(point),
            Some(_) => {}
        }
    }
    let Some(worst) = worst else {
        return Err("SPAN risk array is empty".to_owned());
    };
    if worst.is_sign_negative() {
        Ok((Decimal::ZERO, points))
    } else {
        Ok((worst, points))
    }
}

#[derive(Clone, Debug)]
enum SymbolSpec {
    Future {
        product: String,
        year: i32,
        month: u32,
        day: Option<u32>,
        instrument: Option<String>,
    },
    Option {
        product: String,
        year: i32,
        month: u32,
        day: Option<u32>,
        strike: Decimal,
        call: bool,
        instrument: Option<String>,
    },
    Stock {
        product: String,
    },
}

fn parse_symbol(symbol: &str) -> Result<SymbolSpec, String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.is_empty() {
        return Err("a position symbol is empty".to_owned());
    }
    if symbol.contains(':') {
        return parse_canonical(&symbol);
    }
    if let Some(body) = symbol.strip_suffix("FUT") {
        return parse_future(&symbol, body);
    }
    if let Some(body) = symbol.strip_suffix("CE")
        && body.chars().any(|character| character.is_ascii_digit())
    {
        return parse_option(&symbol, body, true);
    }
    if let Some(body) = symbol.strip_suffix("PE")
        && body.chars().any(|character| character.is_ascii_digit())
    {
        return parse_option(&symbol, body, false);
    }
    if !symbol
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(format!("{symbol} is not a product code"));
    }
    Ok(SymbolSpec::Stock { product: symbol })
}

fn parse_future(symbol: &str, body: &str) -> Result<SymbolSpec, String> {
    if let Some((product, year, month)) = monthly_contract(body) {
        if product.is_empty() {
            return Err(format!("{symbol} does not name a futures product"));
        }
        return Ok(SymbolSpec::Future {
            product,
            year,
            month,
            day: None,
            instrument: None,
        });
    }
    let (product, year, month, day) = dated_contract(symbol, body)?;
    Ok(SymbolSpec::Future {
        product,
        year,
        month,
        day: Some(day),
        instrument: None,
    })
}

fn parse_canonical(symbol: &str) -> Result<SymbolSpec, String> {
    let parts: Vec<&str> = symbol.split(':').collect();
    if parts.first() == Some(&"NSE") {
        return parse_exchange_symbol(symbol, &parts);
    }
    let instrument = parts.first().copied().unwrap_or("");
    match (instrument, parts.len()) {
        ("EQ", 2) => {
            if !ascii_product(parts[1]) {
                return Err(format!("{symbol} is not a canonical stock symbol"));
            }
            Ok(SymbolSpec::Stock {
                product: parts[1].to_owned(),
            })
        }
        ("FUTIDX" | "FUTSTK", 3) => {
            let (year, month, day) = canonical_date(symbol, parts[2])?;
            if !ascii_product(parts[1]) {
                return Err(format!("{symbol} is not a canonical futures symbol"));
            }
            Ok(SymbolSpec::Future {
                product: parts[1].to_owned(),
                year,
                month,
                day: Some(day),
                instrument: Some(instrument.to_owned()),
            })
        }
        ("OPTIDX" | "OPTSTK", 5) => {
            let (year, month, day) = canonical_date(symbol, parts[2])?;
            let call = match parts[4] {
                "CE" => true,
                "PE" => false,
                _ => return Err(format!("{symbol} option type must be CE or PE")),
            };
            if !ascii_product(parts[1]) {
                return Err(format!("{symbol} is not a canonical option symbol"));
            }
            Ok(SymbolSpec::Option {
                product: parts[1].to_owned(),
                year,
                month,
                day: Some(day),
                strike: strike(symbol, parts[3])?,
                call,
                instrument: Some(instrument.to_owned()),
            })
        }
        _ => Err(format!(
            "{symbol} is not a canonical symbol. Use NSE:PRODUCT:YYYY-MM-DD:FUT, NSE:PRODUCT:YYYY-MM-DD:STRIKE:CE, NSE:PRODUCT:YYYY-MM-DD:STRIKE:PE, or NSE:PRODUCT:EQ"
        )),
    }
}

fn parse_exchange_symbol(symbol: &str, parts: &[&str]) -> Result<SymbolSpec, String> {
    match parts {
        ["NSE", product, "EQ"] => {
            if !ascii_product(product) {
                return Err(format!("{symbol} is not a canonical stock symbol"));
            }
            Ok(SymbolSpec::Stock {
                product: (*product).to_owned(),
            })
        }
        ["NSE", product, date, "FUT"] => {
            let (year, month, day) = canonical_date(symbol, date)?;
            if !ascii_product(product) {
                return Err(format!("{symbol} is not a canonical futures symbol"));
            }
            Ok(SymbolSpec::Future {
                product: (*product).to_owned(),
                year,
                month,
                day: Some(day),
                instrument: None,
            })
        }
        ["NSE", product, date, strike_text, right] => {
            let call = match *right {
                "C" | "CE" => true,
                "P" | "PE" => false,
                _ => {
                    return Err(format!("{symbol} option type must be C, P, CE, or PE"));
                }
            };
            let (year, month, day) = canonical_date(symbol, date)?;
            if !ascii_product(product) {
                return Err(format!("{symbol} is not a canonical option symbol"));
            }
            Ok(SymbolSpec::Option {
                product: (*product).to_owned(),
                year,
                month,
                day: Some(day),
                strike: strike(symbol, strike_text)?,
                call,
                instrument: None,
            })
        }
        _ => Err(format!(
            "{symbol} is not a canonical symbol. Use NSE:PRODUCT:YYYY-MM-DD:FUT, NSE:PRODUCT:YYYY-MM-DD:STRIKE:CE, NSE:PRODUCT:YYYY-MM-DD:STRIKE:PE, or NSE:PRODUCT:EQ"
        )),
    }
}

fn canonical_date(symbol: &str, text: &str) -> Result<(i32, u32, u32), String> {
    let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| format!("{symbol} date must be YYYY-MM-DD"))?;
    Ok((date.year(), date.month(), date.day()))
}

fn parse_option(symbol: &str, body: &str, call: bool) -> Result<SymbolSpec, String> {
    if let Some((product, year, month, strike_text)) = monthly_option(body) {
        let strike = strike(symbol, &strike_text)?;
        return Ok(SymbolSpec::Option {
            product,
            year,
            month,
            day: None,
            strike,
            call,
            instrument: None,
        });
    }
    let Some((product, year, month, day, strike)) = weekly_option_splits(body).into_iter().next()
    else {
        return Err(format!("{symbol} is not a monthly or dated option symbol"));
    };
    Ok(SymbolSpec::Option {
        product,
        year,
        month,
        day: Some(day),
        strike,
        call,
        instrument: None,
    })
}

fn monthly_contract(body: &str) -> Option<(String, i32, u32)> {
    if body.len() < 5 {
        return None;
    }
    let month_name = &body[body.len() - 3..];
    let year_text = &body[body.len() - 5..body.len() - 3];
    let product = &body[..body.len() - 5];
    let month = u32::try_from(MONTHS.iter().position(|month| *month == month_name)? + 1).ok()?;
    if product.is_empty() || !ascii_product(product) || !ascii_digits(year_text) {
        return None;
    }
    let year = 2000 + year_text.parse::<i32>().ok()?;
    Some((product.to_owned(), year, month))
}

fn monthly_option(body: &str) -> Option<(String, i32, u32, String)> {
    let mut found = None;
    for (index, month_name) in MONTHS.iter().enumerate() {
        let mut start = 0;
        while let Some(relative) = body[start..].find(month_name) {
            let pos = start + relative;
            start = pos + month_name.len();
            if pos < 2 {
                continue;
            }
            let year_text = &body[pos - 2..pos];
            let product = &body[..pos - 2];
            let strike_text = &body[pos + month_name.len()..];
            if !ascii_digits(year_text) || !ascii_product(product) || strike_text.is_empty() {
                continue;
            }
            let Ok(year) = year_text.parse::<i32>() else {
                continue;
            };
            let month = u32::try_from(index + 1).unwrap_or(0);
            if found.is_some() {
                return None;
            }
            found = Some((
                product.to_owned(),
                2000 + year,
                month,
                strike_text.to_owned(),
            ));
        }
    }
    found
}

fn dated_contract(symbol: &str, body: &str) -> Result<(String, i32, u32, u32), String> {
    if body.len() < 6 {
        return Err(format!(
            "{symbol} does not contain a product, year, month, and day"
        ));
    }
    let day_text = &body[body.len() - 2..];
    let month_code = char::from(body.as_bytes()[body.len() - 3]);
    let year_text = &body[body.len() - 5..body.len() - 3];
    let product = &body[..body.len() - 5];
    let Some(month) = month_from_code(month_code) else {
        return Err(format!("{symbol} does not contain a month code"));
    };
    if !ascii_product(product) || !ascii_digits(year_text) {
        return Err(format!("{symbol} does not name a futures product"));
    }
    let year = 2000
        + year_text
            .parse::<i32>()
            .map_err(|_| format!("{symbol} does not contain a two-digit year"))?;
    let day = day(symbol, day_text)?;
    Ok((product.to_owned(), year, month, day))
}

fn weekly_option_splits(body: &str) -> Vec<(String, i32, u32, u32, Decimal)> {
    let bytes = body.as_bytes();
    let mut found = Vec::new();
    if bytes.len() < 6 {
        return found;
    }
    for index in 0..=bytes.len() - 6 {
        if !bytes[index].is_ascii_digit() || !bytes[index + 1].is_ascii_digit() {
            continue;
        }
        let Some(month) = month_from_code(char::from(bytes[index + 2])) else {
            continue;
        };
        let day_text = &body[index + 3..index + 5];
        let Ok(day) = day_text.parse::<u32>() else {
            continue;
        };
        if !(1..=31).contains(&day) {
            continue;
        }
        let product = &body[..index];
        let strike_text = &body[index + 5..];
        if !ascii_product(product) {
            continue;
        }
        let Ok(strike) = Decimal::from_str(strike_text) else {
            continue;
        };
        if strike_text.is_empty() || strike.is_sign_negative() {
            continue;
        }
        let Ok(year) = body[index..index + 2].parse::<i32>() else {
            continue;
        };
        found.push((product.to_owned(), 2000 + year, month, day, strike));
    }
    found
}

fn strike(symbol: &str, text: &str) -> Result<Decimal, String> {
    let strike = Decimal::from_str(text)
        .map_err(|_| format!("{symbol} does not contain a decimal strike"))?;
    if strike.is_sign_negative() {
        return Err(format!("{symbol} has a negative strike"));
    }
    Ok(strike)
}

fn day(symbol: &str, text: &str) -> Result<u32, String> {
    let day = text
        .parse::<u32>()
        .map_err(|_| format!("{symbol} does not contain a two-digit day"))?;
    if !(1..=31).contains(&day) {
        return Err(format!("{symbol} does not contain a calendar day"));
    }
    Ok(day)
}

fn month_from_code(code: char) -> Option<u32> {
    match code {
        '1'..='9' => {
            let code_byte = u8::try_from(u32::from(code)).ok()?;
            Some(u32::from(code_byte - b'0'))
        }
        'O' => Some(10),
        'N' => Some(11),
        'D' => Some(12),
        _ => None,
    }
}

fn week_code(month: u32) -> char {
    match month {
        1..=9 => {
            let code = u8::try_from(month).unwrap_or(0);
            char::from(b'0' + code)
        }
        10 => 'O',
        11 => 'N',
        12 => 'D',
        _ => '?',
    }
}

fn ascii_product(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

fn ascii_digits(value: &str) -> bool {
    value.len() == 2 && value.chars().all(|character| character.is_ascii_digit())
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

fn expiry_matches(expiry: Option<NaiveDate>, year: i32, month: u32, day: Option<u32>) -> bool {
    let Some(expiry) = expiry else {
        return false;
    };
    expiry.year() == year && expiry.month() == month && day.is_none_or(|day| expiry.day() == day)
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

fn canonical_symbol(contract: &SpanContract) -> String {
    let Some(expiry) = contract.expiry else {
        return format!("NSE:{}:EQ", contract.product);
    };
    canonical_parts(&contract.product, expiry, contract.strike, contract.kind)
}

fn canonical_parts(
    product: &str,
    expiry: NaiveDate,
    strike: Option<Decimal>,
    kind: ContractKind,
) -> String {
    match kind {
        ContractKind::Stock => format!("NSE:{product}:EQ"),
        ContractKind::Future => format!("NSE:{product}:{expiry}:FUT"),
        ContractKind::Call => format!(
            "NSE:{product}:{expiry}:{}:CE",
            strike.unwrap_or(Decimal::ZERO).normalize()
        ),
        ContractKind::Put => format!(
            "NSE:{product}:{expiry}:{}:PE",
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

impl SymbolSpec {
    fn wants_future(&self, product: &str, expiry: NaiveDate) -> bool {
        match self {
            Self::Future {
                product: name,
                year,
                month,
                day,
                instrument: _,
            } => name == product && expiry_matches(Some(expiry), *year, *month, *day),
            Self::Option {
                product: name,
                year,
                month,
                day: None,
                ..
            } => name == product && expiry_matches(Some(expiry), *year, *month, None),
            _ => false,
        }
    }

    fn wants_option(&self, product: &str, call: bool, strike: Decimal, expiry: NaiveDate) -> bool {
        match self {
            Self::Option {
                product: name,
                year,
                month,
                day,
                strike: needed,
                call: needed_call,
                instrument: _,
            } => {
                name == product
                    && *needed_call == call
                    && *needed == strike
                    && expiry_matches(Some(expiry), *year, *month, *day)
            }
            _ => false,
        }
    }

    fn wants_stock(&self, product: &str) -> bool {
        matches!(self, Self::Stock { product: name } if name == product)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Other,
    PointInTime,
    Date,
    FutPf,
    OopPf,
    PhyPf,
    PfCode,
    Fut,
    Series,
    Opt,
    Phy,
    Expiry,
    Price,
    Cvf,
    Scaling,
    Right,
    Strike,
    Ra,
    Value,
    CcDef,
    Cc,
    RiskExponent,
    Combination,
    SomTiers,
    Val,
    Bound,
    Delta,
    Spread,
    SpreadNumber,
    ChargeMethod,
    Leg,
    Ratio,
}

fn classify(name: &[u8]) -> Tag {
    match name {
        b"pointInTime" => Tag::PointInTime,
        b"date" => Tag::Date,
        b"futPf" => Tag::FutPf,
        b"oopPf" => Tag::OopPf,
        b"phyPf" => Tag::PhyPf,
        b"pfCode" => Tag::PfCode,
        b"fut" => Tag::Fut,
        b"series" => Tag::Series,
        b"opt" => Tag::Opt,
        b"phy" => Tag::Phy,
        b"pe" => Tag::Expiry,
        b"p" => Tag::Price,
        b"cvf" => Tag::Cvf,
        b"sc" => Tag::Scaling,
        b"o" => Tag::Right,
        b"k" => Tag::Strike,
        b"ra" => Tag::Ra,
        b"a" => Tag::Value,
        b"ccDef" => Tag::CcDef,
        b"cc" => Tag::Cc,
        b"riskExponent" => Tag::RiskExponent,
        b"cmbMeth" => Tag::Combination,
        b"somTiers" => Tag::SomTiers,
        b"val" => Tag::Val,
        b"sPe" | b"ePe" => Tag::Bound,
        b"d" => Tag::Delta,
        b"dSpread" => Tag::Spread,
        b"spread" => Tag::SpreadNumber,
        b"chargeMeth" => Tag::ChargeMethod,
        b"pLeg" => Tag::Leg,
        b"i" => Tag::Ratio,
        _ => Tag::Other,
    }
}

struct ExtractState<'a> {
    interest: &'a [SymbolSpec],
    business_date: Option<NaiveDate>,
    product: Option<String>,
    current: Option<Tag>,
    commodity_name: Option<String>,
    risk_exponent: Option<i32>,
    combination_method: Option<String>,
    som_rates: Vec<Decimal>,
    som_bounded: bool,
    series_expiry: Option<NaiveDate>,
    series_cvf: Option<Decimal>,
    series_scaling: Option<Decimal>,
    expiry: Option<NaiveDate>,
    expiry_seen: bool,
    price: Option<Decimal>,
    cvf: Option<Decimal>,
    scaling: Option<Decimal>,
    right: Option<char>,
    strike: Option<Decimal>,
    values: Vec<Decimal>,
    saw_array: bool,
    capture: bool,
    delta: Option<Decimal>,
    spread_number: Option<u32>,
    spread_method: Option<String>,
    spread_rate: Option<Decimal>,
    spread_legs: Vec<(String, NaiveDate, Decimal)>,
    leg_product: Option<String>,
    leg_expiry: Option<NaiveDate>,
    leg_ratio: Option<Decimal>,
    contracts: Vec<SpanContract>,
    commodities: BTreeMap<String, Commodity>,
    underlyings: BTreeMap<String, Decimal>,
    spreads: BTreeMap<String, Vec<CalendarSpread>>,
}

fn extract_contracts(
    bytes: &[u8],
    trading_date: NaiveDate,
    interest: &[SymbolSpec],
) -> Result<Book, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack = Vec::new();
    let mut state = ExtractState {
        interest,
        business_date: None,
        product: None,
        current: None,
        commodity_name: None,
        risk_exponent: None,
        combination_method: None,
        som_rates: Vec::new(),
        som_bounded: false,
        series_expiry: None,
        series_cvf: None,
        series_scaling: None,
        expiry: None,
        expiry_seen: false,
        price: None,
        cvf: None,
        scaling: None,
        right: None,
        strike: None,
        values: Vec::new(),
        saw_array: false,
        capture: false,
        delta: None,
        spread_number: None,
        spread_method: None,
        spread_rate: None,
        spread_legs: Vec::new(),
        leg_product: None,
        leg_expiry: None,
        leg_ratio: None,
        contracts: Vec::new(),
        commodities: BTreeMap::new(),
        underlyings: BTreeMap::new(),
        spreads: BTreeMap::new(),
    };
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("SPAN XML could not be read: {error}"))?;
        match event {
            Event::Start(element) => {
                let tag = classify(element.local_name().as_ref());
                open_extract(&mut state, &stack, tag)?;
                stack.push(tag);
            }
            Event::Empty(element) => {
                let tag = classify(element.local_name().as_ref());
                open_extract(&mut state, &stack, tag)?;
                close_extract(&mut state, tag, stack.last().copied())?;
            }
            Event::Text(text) => {
                let value = text
                    .unescape()
                    .map_err(|error| format!("SPAN XML text could not be decoded: {error}"))?;
                if !value.is_empty() {
                    take_extract(&mut state, &stack, &value)?;
                }
            }
            Event::End(element) => {
                let tag = classify(element.local_name().as_ref());
                let opened = stack
                    .pop()
                    .ok_or_else(|| "SPAN XML has an extra end tag".to_owned())?;
                if opened != tag {
                    return Err("SPAN XML end tag does not match the open element".to_owned());
                }
                close_extract(&mut state, tag, stack.last().copied())?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err("SPAN XML ended before all elements were closed".to_owned());
    }
    match state.business_date {
        Some(date) if date == trading_date => {}
        Some(date) => {
            return Err(format!(
                "SPAN business date {date} does not match trading date {trading_date}"
            ));
        }
        None => return Err("SPAN business date is missing".to_owned()),
    }
    Ok(Book {
        contracts: state.contracts,
        commodities: state.commodities,
        underlyings: state.underlyings,
        spreads: state.spreads,
    })
}

fn open_extract(state: &mut ExtractState<'_>, stack: &[Tag], tag: Tag) -> Result<(), String> {
    match tag {
        Tag::FutPf | Tag::OopPf | Tag::PhyPf => state.product = None,
        Tag::Series => {
            state.series_expiry = None;
            state.series_cvf = None;
            state.series_scaling = None;
        }
        Tag::Fut => {
            if stack.last() != Some(&Tag::FutPf) {
                return Err("SPAN future is outside a futures product".to_owned());
            }
            clear_contract(state, Tag::Fut);
        }
        Tag::Phy => {
            if stack.last() != Some(&Tag::PhyPf) {
                return Err("SPAN physical contract is outside a physical product".to_owned());
            }
            clear_contract(state, Tag::Phy);
        }
        Tag::Opt => {
            if stack.last() != Some(&Tag::Series) {
                return Err("SPAN option is outside a series".to_owned());
            }
            clear_contract(state, Tag::Opt);
            state.expiry = state.series_expiry;
            state.expiry_seen = state.series_expiry.is_some();
            state.cvf = state.series_cvf;
            state.scaling = state.series_scaling;
        }
        Tag::Ra if state.current.is_some() => {
            if state.saw_array {
                return Err("SPAN contract has more than one risk array".to_owned());
            }
            state.values.clear();
            state.capture = retained(state);
        }
        Tag::CcDef => {
            state.commodity_name = None;
            state.risk_exponent = None;
            state.combination_method = None;
            state.som_rates.clear();
            state.som_bounded = false;
        }
        Tag::Spread => {
            state.spread_number = None;
            state.spread_method = None;
            state.spread_rate = None;
            state.spread_legs.clear();
        }
        Tag::Leg => {
            state.leg_product = None;
            state.leg_expiry = None;
            state.leg_ratio = None;
        }
        _ => {}
    }
    Ok(())
}

fn clear_contract(state: &mut ExtractState<'_>, current: Tag) {
    state.current = Some(current);
    state.expiry = None;
    state.expiry_seen = false;
    state.price = None;
    state.cvf = None;
    state.scaling = None;
    state.right = None;
    state.strike = None;
    state.values.clear();
    state.saw_array = false;
    state.capture = false;
    state.delta = None;
}

fn close_extract(
    state: &mut ExtractState<'_>,
    tag: Tag,
    parent: Option<Tag>,
) -> Result<(), String> {
    match tag {
        Tag::Fut if parent == Some(Tag::FutPf) => finish_contract(state, ContractKind::Future)?,
        Tag::Phy if parent == Some(Tag::PhyPf) => finish_contract(state, ContractKind::Stock)?,
        Tag::Opt if parent == Some(Tag::Series) => {
            let kind = match state.right {
                Some('C') => ContractKind::Call,
                Some('P') => ContractKind::Put,
                Some(other) => {
                    return Err(format!("SPAN option right {other} is not C or P"));
                }
                None => return Err("SPAN option has no right".to_owned()),
            };
            finish_contract(state, kind)?;
        }
        Tag::CcDef => finish_commodity(state)?,
        Tag::Leg if parent == Some(Tag::Spread) => finish_leg(state)?,
        Tag::Spread => finish_spread(state)?,
        _ => {}
    }
    Ok(())
}

fn finish_contract(state: &mut ExtractState<'_>, kind: ContractKind) -> Result<(), String> {
    let product = state
        .product
        .clone()
        .ok_or_else(|| format!("SPAN {} has no product code", kind.label()))?;
    if kind != ContractKind::Stock && !state.expiry_seen {
        return Err(format!("SPAN {product} {} has no expiry", kind.label()));
    }
    if kind == ContractKind::Stock && !state.expiry_seen {
        return Err(format!("SPAN physical {product} has no expiry"));
    }
    if kind.is_option() && state.strike.is_none() {
        return Err(format!("SPAN option {product} has no strike"));
    }
    let settlement_price = state.price.take().ok_or_else(|| {
        format!(
            "SPAN {product} {} has no settlement price",
            described(kind, state.expiry, state.strike)
        )
    })?;
    if kind == ContractKind::Stock
        && product_wanted(state.interest, &product)
        && state
            .underlyings
            .insert(product.clone(), settlement_price)
            .is_some()
    {
        return Err(format!(
            "SPAN physical {product} underlying price is repeated"
        ));
    }
    let cvf = state.cvf.take().ok_or_else(|| {
        format!(
            "SPAN {product} {} has no contract value factor",
            described(kind, state.expiry, state.strike)
        )
    })?;
    let scaling = state.scaling.take().ok_or_else(|| {
        format!(
            "SPAN {product} {} has no scaling factor",
            described(kind, state.expiry, state.strike)
        )
    })?;
    if !state.saw_array {
        return Err(format!(
            "SPAN {product} {} has no risk array",
            described(kind, state.expiry, state.strike)
        ));
    }
    let keep = match kind {
        ContractKind::Future => state
            .expiry
            .is_some_and(|expiry| wants_future(state.interest, &product, expiry)),
        ContractKind::Call => state.expiry.is_some_and(|expiry| {
            state
                .strike
                .is_some_and(|strike| wants_option(state.interest, &product, true, strike, expiry))
        }),
        ContractKind::Put => state.expiry.is_some_and(|expiry| {
            state
                .strike
                .is_some_and(|strike| wants_option(state.interest, &product, false, strike, expiry))
        }),
        ContractKind::Stock => wants_stock(state.interest, &product),
    };
    if keep {
        if state.values.is_empty() {
            return Err(format!(
                "SPAN {product} {} risk array was not retained",
                kind.label()
            ));
        }
        if state.contracts.iter().any(|contract| {
            contract.kind == kind
                && contract.product == product
                && contract.expiry == state.expiry
                && contract.strike == state.strike
        }) {
            return Err(format!(
                "SPAN {product} {} is repeated",
                described(kind, state.expiry, state.strike)
            ));
        }
        state.contracts.push(SpanContract {
            kind,
            product,
            expiry: state.expiry,
            strike: state.strike,
            settlement_price,
            delta: state.delta.take(),
            cvf,
            scaling,
            values: std::mem::take(&mut state.values),
        });
    } else {
        state.values.clear();
    }
    state.current = None;
    state.saw_array = false;
    state.capture = false;
    Ok(())
}

fn described(kind: ContractKind, expiry: Option<NaiveDate>, strike: Option<Decimal>) -> String {
    match (expiry, strike) {
        (Some(expiry), Some(strike)) => format!("{} {expiry} {strike}", kind.label()),
        (Some(expiry), None) => format!("{} {expiry}", kind.label()),
        (None, _) => kind.label().to_owned(),
    }
}

fn finish_commodity(state: &mut ExtractState<'_>) -> Result<(), String> {
    let name = state
        .commodity_name
        .take()
        .ok_or_else(|| "SPAN combined commodity has no name".to_owned())?;
    let risk_exponent = state
        .risk_exponent
        .take()
        .ok_or_else(|| format!("SPAN combined commodity {name} has no riskExponent"))?;
    let combination_method = state
        .combination_method
        .take()
        .ok_or_else(|| format!("SPAN combined commodity {name} has no combination method"))?;
    let som_is_zero =
        !state.som_bounded && state.som_rates.len() == 1 && state.som_rates[0] == Decimal::ZERO;
    if state
        .commodities
        .insert(
            name.clone(),
            Commodity {
                risk_exponent,
                combination_method,
                som_is_zero,
            },
        )
        .is_some()
    {
        return Err(format!("SPAN combined commodity {name} is repeated"));
    }
    Ok(())
}

fn retained(state: &ExtractState<'_>) -> bool {
    let Some(product) = state.product.as_deref() else {
        return true;
    };
    match state.current {
        Some(Tag::Fut) => state
            .expiry
            .is_some_and(|expiry| wants_future(state.interest, product, expiry)),
        Some(Tag::Opt) => match (state.expiry, state.right, state.strike) {
            (Some(expiry), Some('C'), Some(strike)) => {
                wants_option(state.interest, product, true, strike, expiry)
            }
            (Some(expiry), Some('P'), Some(strike)) => {
                wants_option(state.interest, product, false, strike, expiry)
            }
            _ => true,
        },
        Some(Tag::Phy) => wants_stock(state.interest, product),
        _ => false,
    }
}

fn wants_future(interest: &[SymbolSpec], product: &str, expiry: NaiveDate) -> bool {
    interest
        .iter()
        .any(|spec| spec.wants_future(product, expiry))
}

fn wants_option(
    interest: &[SymbolSpec],
    product: &str,
    call: bool,
    strike: Decimal,
    expiry: NaiveDate,
) -> bool {
    interest
        .iter()
        .any(|spec| spec.wants_option(product, call, strike, expiry))
}

fn wants_stock(interest: &[SymbolSpec], product: &str) -> bool {
    interest.iter().any(|spec| spec.wants_stock(product))
}

fn take_extract(state: &mut ExtractState<'_>, stack: &[Tag], value: &str) -> Result<(), String> {
    let parent = stack.last().copied().unwrap_or(Tag::Other);
    let grandparent = stack.get(stack.len().wrapping_sub(2)).copied();
    match parent {
        Tag::Date if grandparent == Some(Tag::PointInTime) => {
            let parsed = NaiveDate::parse_from_str(value, "%Y%m%d")
                .map_err(|_| format!("SPAN business date `{value}` is not YYYYMMDD"))?;
            if state.business_date.replace(parsed).is_some() {
                return Err("SPAN business date is repeated".to_owned());
            }
        }
        Tag::PfCode if matches!(grandparent, Some(Tag::FutPf | Tag::OopPf | Tag::PhyPf)) => {
            state.product = Some(value.to_owned());
        }
        Tag::Expiry if grandparent == Some(Tag::Leg) => {
            state.leg_expiry = Some(
                NaiveDate::parse_from_str(value, "%Y%m%d")
                    .map_err(|_| format!("SPAN spread expiry `{value}` is not YYYYMMDD"))?,
            );
        }
        Tag::Expiry if matches!(grandparent, Some(Tag::Fut | Tag::Phy)) => {
            if grandparent == Some(Tag::Phy) && value == "00000000" {
                state.expiry = None;
                state.expiry_seen = true;
            } else {
                state.expiry = Some(
                    NaiveDate::parse_from_str(value, "%Y%m%d")
                        .map_err(|_| format!("SPAN expiry `{value}` is not YYYYMMDD"))?,
                );
                state.expiry_seen = true;
            }
        }
        Tag::Expiry if grandparent == Some(Tag::Series) => {
            state.series_expiry = Some(
                NaiveDate::parse_from_str(value, "%Y%m%d")
                    .map_err(|_| format!("SPAN option series expiry `{value}` is not YYYYMMDD"))?,
            );
        }
        Tag::Price if matches!(grandparent, Some(Tag::Fut | Tag::Phy | Tag::Opt)) => {
            state.price = Some(decimal(value, "settlement price")?);
        }
        Tag::Cvf if grandparent == Some(Tag::Series) => {
            state.series_cvf = Some(decimal(value, "contract value factor")?);
        }
        Tag::Cvf if matches!(grandparent, Some(Tag::Fut | Tag::Phy | Tag::Opt)) => {
            state.cvf = Some(decimal(value, "contract value factor")?);
        }
        Tag::Scaling if grandparent == Some(Tag::Series) => {
            state.series_scaling = Some(decimal(value, "scaling factor")?);
        }
        Tag::Scaling if matches!(grandparent, Some(Tag::Fut | Tag::Phy | Tag::Opt)) => {
            state.scaling = Some(decimal(value, "scaling factor")?);
        }
        Tag::Right if grandparent == Some(Tag::Opt) => {
            state.right = Some(match value {
                "C" => 'C',
                "P" => 'P',
                _ => return Err(format!("SPAN option right `{value}` is not C or P")),
            });
        }
        Tag::Strike if grandparent == Some(Tag::Opt) => {
            state.strike = Some(decimal(value, "strike")?);
        }
        Tag::Value if grandparent == Some(Tag::Ra) && state.current.is_some() => {
            let number = decimal(value, "risk array value")?;
            state.saw_array = true;
            if state.capture {
                state.values.push(number);
            }
        }
        Tag::Cc if grandparent == Some(Tag::Leg) => {
            state.leg_product = Some(value.to_owned());
        }
        Tag::Cc if grandparent == Some(Tag::CcDef) => {
            state.commodity_name = Some(value.to_owned());
        }
        Tag::RiskExponent if grandparent == Some(Tag::CcDef) => {
            state.risk_exponent = Some(
                value
                    .parse::<i32>()
                    .map_err(|_| format!("SPAN riskExponent `{value}` is not an integer"))?,
            );
        }
        Tag::Combination if grandparent == Some(Tag::CcDef) => {
            state.combination_method = Some(value.to_owned());
        }
        Tag::Val if stack.contains(&Tag::Spread) => {
            if state
                .spread_rate
                .replace(decimal(value, "spread charge")?)
                .is_some()
            {
                return Err("SPAN calendar spread has more than one charge".to_owned());
            }
        }
        Tag::Val if stack.contains(&Tag::SomTiers) => {
            state
                .som_rates
                .push(decimal(value, "short option minimum")?);
        }
        Tag::Bound if stack.contains(&Tag::SomTiers) => state.som_bounded = true,
        Tag::Delta if matches!(grandparent, Some(Tag::Fut | Tag::Opt | Tag::Phy)) => {
            state.delta = Some(decimal(value, "delta")?);
        }
        Tag::SpreadNumber => {
            state.spread_number = Some(
                value
                    .parse::<u32>()
                    .map_err(|_| format!("SPAN spread number `{value}` is not an integer"))?,
            );
        }
        Tag::ChargeMethod => state.spread_method = Some(value.to_owned()),
        Tag::Ratio if grandparent == Some(Tag::Leg) => {
            state.leg_ratio = Some(decimal(value, "spread leg ratio")?);
        }
        _ => {}
    }
    Ok(())
}

fn finish_leg(state: &mut ExtractState<'_>) -> Result<(), String> {
    let product = state
        .leg_product
        .take()
        .ok_or_else(|| "SPAN spread leg has no combined commodity".to_owned())?;
    let expiry = state
        .leg_expiry
        .take()
        .ok_or_else(|| format!("SPAN spread leg for {product} has no expiry"))?;
    let ratio = state
        .leg_ratio
        .take()
        .ok_or_else(|| format!("SPAN spread leg for {product} has no ratio"))?;
    state.spread_legs.push((product, expiry, ratio));
    Ok(())
}

fn finish_spread(state: &mut ExtractState<'_>) -> Result<(), String> {
    let legs = std::mem::take(&mut state.spread_legs);
    if legs.is_empty() {
        state.spread_number = None;
        state.spread_method = None;
        state.spread_rate = None;
        return Ok(());
    }
    let wanted = legs
        .iter()
        .any(|(name, _, _)| product_wanted(state.interest, name));
    if !wanted {
        state.spread_number = None;
        state.spread_method = None;
        state.spread_rate = None;
        return Ok(());
    }
    if legs.len() != 2 || legs[0].0 != legs[1].0 {
        return Err(
            "SPAN intercommodity spread is not applied, and a calendar spread needs two legs of one product"
                .to_owned(),
        );
    }
    let product = legs[0].0.clone();
    let number = state
        .spread_number
        .take()
        .ok_or_else(|| format!("SPAN calendar spread for {product} has no number"))?;
    let method = state
        .spread_method
        .take()
        .ok_or_else(|| format!("SPAN calendar spread for {product} has no charge method"))?;
    let charge = state
        .spread_rate
        .take()
        .ok_or_else(|| format!("SPAN calendar spread for {product} has no charge"))?;
    let spreads = state.spreads.entry(product.clone()).or_default();
    if spreads.iter().any(|spread| spread.number == number) {
        return Err(format!(
            "SPAN calendar spread {number} for {product} is repeated"
        ));
    }
    spreads.push(CalendarSpread {
        number,
        charge,
        method,
        legs: [(legs[0].1, legs[0].2), (legs[1].1, legs[1].2)],
    });
    spreads.sort_by_key(|spread| spread.number);
    Ok(())
}

fn decimal(value: &str, what: &str) -> Result<Decimal, String> {
    Decimal::from_str(value).map_err(|_| format!("SPAN {what} `{value}` is not a decimal number"))
}

fn product_wanted(interest: &[SymbolSpec], product: &str) -> bool {
    interest.iter().any(|spec| spec.product() == product)
}

impl SymbolSpec {
    fn product(&self) -> &str {
        match self {
            Self::Future { product, .. }
            | Self::Option { product, .. }
            | Self::Stock { product } => product,
        }
    }
}

const INDEX_ELM_PERCENT: u32 = 2;
const INDEX_DEEP_OTM_PERCENT: u32 = 3;
const INDEX_LONG_PERCENT: u32 = 5;
const INDEX_EXPIRY_EXTRA_PERCENT: u32 = 2;

#[derive(Clone, Debug)]
pub struct ExposureRates {
    stocks: BTreeMap<String, StockElm>,
}

#[derive(Clone, Copy, Debug)]
struct StockElm {
    oth_percent: Decimal,
    otm_percent: Decimal,
}

impl ExposureRates {
    pub fn parse_csv(bytes: &[u8], trading_date: NaiveDate) -> Result<Self, String> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| "exposure file is not UTF-8".to_owned())?;
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default().trim_end_matches('\r');
        let expected = format!(
            "Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date {},Total applicable ELM%",
            crate::nse_reports::display_date(trading_date)
        );
        if header != expected {
            return Err(
                "exposure file header does not match the verified NSE column line".to_owned(),
            );
        }
        let mut partial: BTreeMap<String, (Option<Decimal>, Option<Decimal>)> = BTreeMap::new();
        for (index, line) in lines.enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() != 6 {
                return Err(format!(
                    "exposure file row {} does not have six columns",
                    index + 2
                ));
            }
            let symbol = fields[1].trim().to_ascii_uppercase();
            if symbol.is_empty() {
                return Err(format!("exposure file row {} has no symbol", index + 2));
            }
            let normal = elm_percent(fields[3], index + 2, "normal")?;
            let additional = elm_percent(fields[4], index + 2, "additional")?;
            let total = elm_percent(fields[5], index + 2, "total")?;
            if normal
                .checked_add(additional)
                .is_none_or(|sum| sum != total)
            {
                return Err(format!(
                    "exposure file total for {symbol} is not the normal percent plus the additional percent"
                ));
            }
            let entry = partial.entry(symbol.clone()).or_default();
            let slot = match fields[2].trim() {
                "OTH" => &mut entry.0,
                "OTM" => &mut entry.1,
                other => {
                    return Err(format!(
                        "exposure file instrument type `{other}` for {symbol} is not OTH or OTM"
                    ));
                }
            };
            if slot.is_some() {
                return Err(format!(
                    "exposure file repeats {} for {symbol}",
                    fields[2].trim()
                ));
            }
            *slot = Some(total);
        }
        if partial.is_empty() {
            return Err("exposure file has no data row".to_owned());
        }
        let mut stocks = BTreeMap::new();
        for (symbol, (oth, otm)) in partial {
            let (Some(oth_percent), Some(otm_percent)) = (oth, otm) else {
                return Err(format!(
                    "exposure file {symbol} does not have both an OTH row and an OTM row"
                ));
            };
            stocks.insert(
                symbol,
                StockElm {
                    oth_percent,
                    otm_percent,
                },
            );
        }
        Ok(Self { stocks })
    }

    fn contains_stock(&self, product: &str) -> bool {
        self.stocks.contains_key(product)
    }

    fn future_percent(&self, product: &str) -> Decimal {
        self.stocks
            .get(product)
            .map(|stock| stock.oth_percent)
            .unwrap_or_else(|| Decimal::from(INDEX_ELM_PERCENT))
    }
}

fn elm_percent(text: &str, row: usize, label: &str) -> Result<Decimal, String> {
    let value = Decimal::from_str(text.trim()).map_err(|_| {
        format!("exposure file row {row} {label} percent `{text}` is not a decimal number")
    })?;
    if value.is_sign_negative() {
        return Err(format!(
            "exposure file row {row} {label} percent is negative"
        ));
    }
    Ok(value)
}

struct ExtremeLossInput<'a> {
    product: &'a str,
    kind: ContractKind,
    quantity: Decimal,
    settlement_price: Decimal,
    strike: Option<Decimal>,
    expiry: Option<NaiveDate>,
    underlying: Option<Decimal>,
    trading_date: NaiveDate,
    exposure: &'a ExposureRates,
    contracts: &'a ContractElmRates,
}

struct ExtremeLoss {
    amount: Decimal,
    percent: Decimal,
    underlying: Option<Decimal>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContractElmRates {
    rows: BTreeMap<(String, NaiveDate, ContractKind, Option<Decimal>), Decimal>,
    listed: Vec<ListedContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedContract {
    instrument: String,
    product: String,
    expiry: NaiveDate,
    strike: Option<Decimal>,
    kind: ContractKind,
}

impl ContractElmRates {
    pub fn parse_csv(bytes: &[u8]) -> Result<Self, String> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| "contract ELM file is not UTF-8".to_owned())?;
        let mut rows = BTreeMap::new();
        let mut listed = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let row = index + 1;
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() != 7 {
                return Err(format!(
                    "contract ELM file row {row} does not have 7 fields"
                ));
            }
            let instrument = fields[0].trim().to_owned();
            let kind = contract_kind(&instrument, fields[4].trim(), row)?;
            let product = fields[1].trim();
            if product.is_empty() {
                return Err(format!("contract ELM file row {row} has no symbol"));
            }
            let expiry = contract_date(fields[2].trim(), row)?;
            let strike = contract_strike(fields[3].trim(), kind, row)?;
            let level = fields[5].trim();
            if level != "0" {
                return Err(format!(
                    "contract ELM file row {row} corporate action level `{level}` is not applied"
                ));
            }
            let percent = Decimal::from_str(fields[6].trim()).map_err(|_| {
                format!("contract ELM file row {row} percent is not a decimal number")
            })?;
            if percent.is_sign_negative() {
                return Err(format!("contract ELM file row {row} percent is negative"));
            }
            let key = (product.to_owned(), expiry, kind, strike);
            if let Some(existing) = rows.get(&key) {
                if existing != &percent {
                    return Err(format!(
                        "contract ELM file repeats {product} {expiry} with different percents"
                    ));
                }
            } else {
                listed.push(ListedContract {
                    instrument,
                    product: product.to_owned(),
                    expiry,
                    strike,
                    kind,
                });
            }
            rows.insert(key, percent);
        }
        if rows.is_empty() {
            return Err("contract ELM file has no rows".to_owned());
        }
        Ok(Self { rows, listed })
    }

    pub fn parse_gzip(bytes: &[u8]) -> Result<Self, String> {
        let mut decoder = GzDecoder::new(bytes);
        let mut csv = Vec::new();
        decoder
            .read_to_end(&mut csv)
            .map_err(|_| "contract ELM file is not valid gzip".to_owned())?;
        Self::parse_csv(&csv)
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn percent(
        &self,
        product: &str,
        kind: ContractKind,
        expiry: NaiveDate,
        strike: Option<Decimal>,
    ) -> Result<Decimal, String> {
        self.rows
            .get(&(product.to_owned(), expiry, kind, strike))
            .copied()
            .ok_or_else(|| {
                format!(
                    "contract ELM file has no rate for {product} {kind:?} {expiry} strike {strike:?}"
                )
            })
    }

    fn identify(
        &self,
        positions: &[MarginPosition],
        specs: Vec<SymbolSpec>,
    ) -> Result<Vec<SymbolSpec>, String> {
        if self.listed.is_empty() {
            return Ok(specs);
        }
        let mut identified = Vec::with_capacity(specs.len());
        for (position, spec) in positions.iter().zip(specs) {
            identified.push(self.identify_one(&position.symbol, spec)?);
        }
        Ok(identified)
    }

    fn identify_one(&self, symbol: &str, spec: SymbolSpec) -> Result<SymbolSpec, String> {
        if matches!(spec, SymbolSpec::Stock { .. }) {
            return Ok(spec);
        }
        let matches = self
            .listed
            .iter()
            .filter(|row| row.matches(&spec))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [row] => Ok(row.pin(spec)),
            [] => Err(format!("{symbol} is not in the contract file")),
            many => {
                let mut names = many.iter().map(|row| row.canonical()).collect::<Vec<_>>();
                names.sort();
                names.dedup();
                Err(format!(
                    "{symbol} matches more than one contract. Use a canonical symbol: {}",
                    names.join(", ")
                ))
            }
        }
    }
}

impl ListedContract {
    fn matches(&self, spec: &SymbolSpec) -> bool {
        match spec {
            SymbolSpec::Future {
                product,
                year,
                month,
                day,
                instrument,
            } => {
                self.kind == ContractKind::Future
                    && self.product == *product
                    && expiry_matches(Some(self.expiry), *year, *month, *day)
                    && instrument
                        .as_ref()
                        .is_none_or(|code| code == &self.instrument)
            }
            SymbolSpec::Option {
                product,
                year,
                month,
                day,
                strike,
                call,
                instrument,
            } => {
                let kind = if *call {
                    ContractKind::Call
                } else {
                    ContractKind::Put
                };
                self.kind == kind
                    && self.product == *product
                    && self.strike == Some(*strike)
                    && expiry_matches(Some(self.expiry), *year, *month, *day)
                    && instrument
                        .as_ref()
                        .is_none_or(|code| code == &self.instrument)
            }
            SymbolSpec::Stock { .. } => false,
        }
    }

    fn pin(&self, spec: SymbolSpec) -> SymbolSpec {
        match spec {
            SymbolSpec::Future {
                product,
                year,
                month,
                ..
            } => SymbolSpec::Future {
                product,
                year,
                month,
                day: Some(self.expiry.day()),
                instrument: Some(self.instrument.clone()),
            },
            SymbolSpec::Option {
                product,
                year,
                month,
                strike,
                call,
                ..
            } => SymbolSpec::Option {
                product,
                year,
                month,
                day: Some(self.expiry.day()),
                strike,
                call,
                instrument: Some(self.instrument.clone()),
            },
            other => other,
        }
    }

    fn canonical(&self) -> String {
        canonical_parts(&self.product, self.expiry, self.strike, self.kind)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CashMarginRates {
    rates: BTreeMap<String, Decimal>,
}

impl CashMarginRates {
    pub fn parse(bytes: &[u8], trading_date: NaiveDate) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "cash-market VaR file is not UTF-8".to_owned())?;
        let expected_date = trading_date.format("%d%m%Y").to_string();
        let mut declared: Option<usize> = None;
        let mut details = 0usize;
        let mut rates = BTreeMap::new();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim().trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let row = index + 1;
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            match fields.first().copied() {
                Some("10") if declared.is_none() => {
                    if fields.len() != 5 {
                        return Err(format!(
                            "cash-market VaR file row {row} does not have 5 control fields"
                        ));
                    }
                    if fields[1] != expected_date {
                        return Err(format!(
                            "cash-market VaR file date {} is not {trading_date}",
                            fields[1]
                        ));
                    }
                    declared = Some(fields[4].parse::<usize>().map_err(|_| {
                        format!("cash-market VaR file row {row} record count is not a number")
                    })?);
                }
                Some("10") => {
                    return Err("cash-market VaR file has more than one control record".to_owned());
                }
                Some("20") => {
                    if declared.is_none() {
                        return Err(
                            "cash-market VaR file detail record appears before the control record"
                                .to_owned(),
                        );
                    }
                    if fields.len() != 10 {
                        return Err(format!(
                            "cash-market VaR file row {row} does not have 10 fields"
                        ));
                    }
                    details += 1;
                    if fields[2] != "EQ" {
                        continue;
                    }
                    let symbol = fields[1].to_ascii_uppercase();
                    if symbol.is_empty() {
                        return Err(format!("cash-market VaR file row {row} has no symbol"));
                    }
                    let percent = Decimal::from_str(fields[9]).map_err(|_| {
                        format!("cash-market VaR file row {row} daily margin rate is not a decimal")
                    })?;
                    if percent.is_sign_negative() {
                        return Err(format!(
                            "cash-market VaR file row {row} daily margin rate is negative"
                        ));
                    }
                    if let Some(existing) = rates.get(&symbol)
                        && existing != &percent
                    {
                        return Err(format!(
                            "cash-market VaR file repeats {symbol} EQ with different daily margin rates"
                        ));
                    }
                    rates.insert(symbol, percent);
                }
                _ => {
                    return Err(format!(
                        "cash-market VaR file row {row} record type is not 10 or 20"
                    ));
                }
            }
        }
        let Some(declared) = declared else {
            return Err("cash-market VaR file has no control record".to_owned());
        };
        if declared != details {
            return Err(format!(
                "cash-market VaR file declares {declared} detail records and contains {details}"
            ));
        }
        if rates.is_empty() {
            return Err("cash-market VaR file has no EQ series".to_owned());
        }
        Ok(Self { rates })
    }

    fn daily_percent(&self, product: &str) -> Result<Decimal, String> {
        if self.rates.is_empty() {
            return Err(format!(
                "delivery margin for {product} is inside the expiry window, and the capital-market VaR rate is not in the archived files"
            ));
        }
        self.rates.get(product).copied().ok_or_else(|| {
            format!(
                "delivery margin for {product} has no EQ series rate in the archived cash-market VaR file"
            )
        })
    }
}

fn contract_kind(instrument: &str, right: &str, row: usize) -> Result<ContractKind, String> {
    match (instrument, right) {
        ("FUTIDX" | "FUTSTK", "XX") => Ok(ContractKind::Future),
        ("OPTIDX" | "OPTSTK", "CE") => Ok(ContractKind::Call),
        ("OPTIDX" | "OPTSTK", "PE") => Ok(ContractKind::Put),
        _ => Err(format!(
            "contract ELM file row {row} instrument `{instrument}` and option type `{right}` are not applied"
        )),
    }
}

fn contract_date(text: &str, row: usize) -> Result<NaiveDate, String> {
    let parts: Vec<&str> = text.split('-').collect();
    if parts.len() != 3 {
        return Err(format!(
            "contract ELM file row {row} expiry `{text}` is not DD-MMM-YYYY"
        ));
    }
    let day = parts[0]
        .parse::<u32>()
        .map_err(|_| format!("contract ELM file row {row} expiry `{text}` is not DD-MMM-YYYY"))?;
    let month = match parts[1] {
        "JAN" => 1,
        "FEB" => 2,
        "MAR" => 3,
        "APR" => 4,
        "MAY" => 5,
        "JUN" => 6,
        "JUL" => 7,
        "AUG" => 8,
        "SEP" => 9,
        "OCT" => 10,
        "NOV" => 11,
        "DEC" => 12,
        _ => {
            return Err(format!(
                "contract ELM file row {row} expiry `{text}` is not DD-MMM-YYYY"
            ));
        }
    };
    let year = parts[2]
        .parse::<i32>()
        .map_err(|_| format!("contract ELM file row {row} expiry `{text}` is not DD-MMM-YYYY"))?;
    NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| format!("contract ELM file row {row} expiry `{text}` is not a date"))
}

fn contract_strike(
    strike: &str,
    kind: ContractKind,
    row: usize,
) -> Result<Option<Decimal>, String> {
    let value = Decimal::from_str(strike)
        .map_err(|_| format!("contract ELM file row {row} strike `{strike}` is not a decimal"))?;
    match kind {
        ContractKind::Future if value == Decimal::from(-1) => Ok(None),
        ContractKind::Call | ContractKind::Put if value.is_sign_positive() => Ok(Some(value)),
        _ => Err(format!(
            "contract ELM file row {row} strike `{strike}` does not match the instrument"
        )),
    }
}

fn published_or_contract(
    contracts: &ContractElmRates,
    product: &str,
    kind: ContractKind,
    expiry: Option<NaiveDate>,
    strike: Option<Decimal>,
    published: Decimal,
) -> Result<Decimal, String> {
    if contracts.is_empty() {
        return Ok(published);
    }
    let expiry = expiry.ok_or_else(|| {
        format!("SPAN {product} has no expiry, so the contract ELM file cannot be read")
    })?;
    contracts.percent(product, kind, expiry, strike)
}

fn contract_option_percent(
    input: &ExtremeLossInput<'_>,
    strike: Decimal,
    underlying: Decimal,
    expiry: NaiveDate,
) -> Result<Decimal, String> {
    if !input.contracts.is_empty() {
        return input
            .contracts
            .percent(input.product, input.kind, expiry, Some(strike));
    }
    short_option_percent(ShortOptionRate {
        exposure: input.exposure,
        product: input.product,
        kind: input.kind,
        strike,
        underlying,
        expiry,
        trading_date: input.trading_date,
    })
}

#[derive(Default)]
struct FormedSpreads {
    charge: Decimal,
    far_qty: BTreeMap<NaiveDate, Decimal>,
    near_qty: BTreeMap<NaiveDate, Decimal>,
}

fn scan_positions<'a>(
    positions: impl Iterator<Item = &'a Requested<'a>>,
    width: usize,
    product: &str,
) -> Result<(Decimal, Vec<u32>), String> {
    let mut losses = vec![Decimal::ZERO; width];
    let mut any = false;
    for position in positions {
        any = true;
        for (index, value) in position.contract.values.iter().enumerate() {
            let loss = position
                .quantity
                .checked_mul(*value)
                .ok_or_else(|| format!("scenario loss for {} overflowed", position.symbol))?;
            losses[index] = losses[index]
                .checked_add(loss)
                .ok_or_else(|| format!("scenario loss for {product} overflowed"))?;
        }
    }
    if !any {
        return Ok((Decimal::ZERO, Vec::new()));
    }
    greatest_loss(&losses)
}

fn form_calendar_spreads(
    product: &str,
    group: &[&Requested<'_>],
    spreads: &[CalendarSpread],
    trading_date: NaiveDate,
    index: bool,
) -> Result<FormedSpreads, String> {
    if spreads.is_empty() {
        return Ok(FormedSpreads::default());
    }
    let mut delta_left = BTreeMap::new();
    let mut future_left = BTreeMap::new();
    for position in group {
        let Some(expiry) = position.contract.expiry else {
            continue;
        };
        let delta = position.contract.delta.ok_or_else(|| {
            format!(
                "SPAN {} has no delta, so the calendar spread is not calculated",
                position.symbol
            )
        })?;
        let signed = position
            .quantity
            .checked_mul(delta)
            .ok_or_else(|| format!("SPAN delta for {} overflowed", position.symbol))?;
        add_expiry(&mut delta_left, expiry, signed)?;
        if position.contract.kind == ContractKind::Future {
            add_expiry(&mut future_left, expiry, position.quantity)?;
        }
    }
    let mut formed = FormedSpreads::default();
    for spread in spreads {
        if spread.method != "F" {
            return Err(format!(
                "SPAN spread charge method {} for {product} is not applied",
                spread.method
            ));
        }
        let [(expiry_a, ratio_a), (expiry_b, ratio_b)] = spread.legs;
        if index && (expiry_a == trading_date || expiry_b == trading_date) {
            continue;
        }
        if ratio_a != Decimal::ONE || ratio_b != Decimal::ONE {
            return Err(format!("SPAN spread ratios for {product} are not both 1"));
        }
        let delta_spreads = consume(&mut delta_left, expiry_a, expiry_b)?;
        let charge = delta_spreads
            .checked_mul(spread.charge)
            .ok_or_else(|| format!("SPAN spread charge for {product} overflowed"))?;
        formed.charge = formed
            .charge
            .checked_add(charge)
            .ok_or_else(|| format!("SPAN spread charge for {product} overflowed"))?;
        let futures = consume(&mut future_left, expiry_a, expiry_b)?;
        if futures.is_sign_positive() {
            let (near, far) = if expiry_a <= expiry_b {
                (expiry_a, expiry_b)
            } else {
                (expiry_b, expiry_a)
            };
            add_expiry(&mut formed.far_qty, far, futures)?;
            add_expiry(&mut formed.near_qty, near, futures)?;
        }
    }
    Ok(formed)
}

fn add_expiry(
    totals: &mut BTreeMap<NaiveDate, Decimal>,
    expiry: NaiveDate,
    quantity: Decimal,
) -> Result<(), String> {
    let entry = totals.entry(expiry).or_default();
    *entry = entry
        .checked_add(quantity)
        .ok_or_else(|| "calendar-spread quantity overflowed".to_owned())?;
    Ok(())
}

fn consume(
    left: &mut BTreeMap<NaiveDate, Decimal>,
    first: NaiveDate,
    second: NaiveDate,
) -> Result<Decimal, String> {
    let first_qty = left.get(&first).copied().unwrap_or(Decimal::ZERO);
    let second_qty = left.get(&second).copied().unwrap_or(Decimal::ZERO);
    if first_qty.is_zero()
        || second_qty.is_zero()
        || first_qty.is_sign_negative() == second_qty.is_sign_negative()
    {
        return Ok(Decimal::ZERO);
    }
    let spreads = first_qty.abs().min(second_qty.abs());
    left.insert(first, shrink(first_qty, spreads)?);
    left.insert(second, shrink(second_qty, spreads)?);
    Ok(spreads)
}

fn shrink(quantity: Decimal, spreads: Decimal) -> Result<Decimal, String> {
    let updated = if quantity.is_sign_negative() {
        quantity.checked_add(spreads)
    } else {
        quantity.checked_sub(spreads)
    };
    updated.ok_or_else(|| "calendar-spread quantity overflowed".to_owned())
}

fn future_extreme_loss(
    product: &str,
    position: &Requested<'_>,
    formed: &FormedSpreads,
    exposure: &ExposureRates,
    contracts: &ContractElmRates,
) -> Result<ExtremeLoss, String> {
    let expiry = position.contract.expiry.ok_or_else(|| {
        format!(
            "SPAN {} future has no expiry, so extreme loss margin is not calculated",
            position.symbol
        )
    })?;
    let percent = published_or_contract(
        contracts,
        product,
        ContractKind::Future,
        Some(expiry),
        None,
        exposure.future_percent(product),
    )?;
    let far = formed
        .far_qty
        .get(&expiry)
        .copied()
        .unwrap_or(Decimal::ZERO);
    let near = formed
        .near_qty
        .get(&expiry)
        .copied()
        .unwrap_or(Decimal::ZERO);
    let residual = position
        .quantity
        .abs()
        .checked_sub(far)
        .and_then(|amount| amount.checked_sub(near))
        .ok_or_else(|| format!("extreme loss margin for {} overflowed", position.symbol))?;
    if residual.is_sign_negative() {
        return Err(format!(
            "calendar spread consumed more futures quantity than {}",
            position.symbol
        ));
    }
    let spread_amount = apply_percent(position.contract.settlement_price, far, percent)?
        .checked_div(Decimal::from(3))
        .ok_or_else(|| "extreme loss margin overflowed".to_owned())?;
    let residual_amount = apply_percent(position.contract.settlement_price, residual, percent)?;
    Ok(ExtremeLoss {
        amount: spread_amount
            .checked_add(residual_amount)
            .ok_or_else(|| "extreme loss margin overflowed".to_owned())?,
        percent,
        underlying: None,
    })
}

fn product_delivery(
    product: &str,
    group: &[&Requested<'_>],
    underlying: Option<Decimal>,
    trading_date: NaiveDate,
    index: bool,
    cash: &CashMarginRates,
) -> Result<Decimal, String> {
    if index {
        return Ok(Decimal::ZERO);
    }
    let mut buckets: BTreeMap<NaiveDate, (Decimal, Decimal)> = BTreeMap::new();
    for position in group {
        if position.contract.kind == ContractKind::Stock {
            continue;
        }
        let Some(expiry) = position.contract.expiry else {
            continue;
        };
        let Some(stage) = weekdays_until(trading_date, expiry) else {
            continue;
        };
        let shares = deliverable_shares(position, underlying)?;
        let entry = buckets
            .entry(expiry)
            .or_insert((Decimal::ZERO, delivery_stage_percent(stage)));
        entry.0 = entry
            .0
            .checked_add(shares)
            .ok_or_else(|| format!("delivery margin for {product} overflowed"))?;
    }
    let mut total = Decimal::ZERO;
    for (shares, stage_percent) in buckets.into_values() {
        if shares.is_zero() {
            continue;
        }
        let price = underlying
            .filter(|price| price.is_sign_positive())
            .ok_or_else(|| {
                format!(
                    "SPAN {product} has no underlying price, so delivery margin is not calculated"
                )
            })?;
        let rate = cash.daily_percent(product)?;
        let amount = shares
            .abs()
            .checked_mul(price)
            .and_then(|notional| notional.checked_mul(rate))
            .and_then(|scaled| scaled.checked_mul(stage_percent))
            .and_then(|scaled| scaled.checked_div(Decimal::from(10_000)))
            .ok_or_else(|| format!("delivery margin for {product} overflowed"))?;
        total = total
            .checked_add(amount)
            .ok_or_else(|| format!("delivery margin for {product} overflowed"))?;
    }
    Ok(total)
}

fn deliverable_shares(
    position: &Requested<'_>,
    underlying: Option<Decimal>,
) -> Result<Decimal, String> {
    match position.contract.kind {
        ContractKind::Stock => Ok(Decimal::ZERO),
        ContractKind::Future => Ok(position.quantity),
        ContractKind::Call | ContractKind::Put => {
            let strike = position.contract.strike.ok_or_else(|| {
                format!(
                    "delivery margin for {} has no strike, so in-the-money status is not known",
                    position.symbol
                )
            })?;
            let price = underlying
                .filter(|price| price.is_sign_positive())
                .ok_or_else(|| {
                    format!(
                        "SPAN {} has no underlying price, so delivery margin is not calculated",
                        position.contract.product
                    )
                })?;
            let in_the_money = match position.contract.kind {
                ContractKind::Call => strike < price,
                ContractKind::Put => strike > price,
                ContractKind::Future | ContractKind::Stock => false,
            };
            if !in_the_money {
                return Ok(Decimal::ZERO);
            }
            if position.contract.kind == ContractKind::Call {
                Ok(position.quantity)
            } else {
                Decimal::ZERO
                    .checked_sub(position.quantity)
                    .ok_or_else(|| format!("delivery margin for {} overflowed", position.symbol))
            }
        }
    }
}

fn delivery_stage_percent(stage: u32) -> Decimal {
    Decimal::from(match stage {
        0 => 100,
        1 => 70,
        2 => 45,
        3 => 25,
        _ => 10,
    })
}

fn weekdays_until(from: NaiveDate, until: NaiveDate) -> Option<u32> {
    if until < from {
        return None;
    }
    let mut cursor = from;
    let mut days = 0;
    while cursor < until {
        cursor = cursor.succ_opt()?;
        if !matches!(cursor.weekday(), Weekday::Sat | Weekday::Sun) {
            days += 1;
        }
        if days > 4 {
            return None;
        }
    }
    Some(days)
}

fn paise(amount: Decimal) -> Decimal {
    amount.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

fn extreme_loss(input: ExtremeLossInput<'_>) -> Result<ExtremeLoss, String> {
    match input.kind {
        ContractKind::Stock => Ok(ExtremeLoss {
            amount: Decimal::ZERO,
            percent: Decimal::ZERO,
            underlying: input.underlying,
        }),
        ContractKind::Future => {
            let percent = published_or_contract(
                input.contracts,
                input.product,
                input.kind,
                input.expiry,
                None,
                input.exposure.future_percent(input.product),
            )?;
            Ok(ExtremeLoss {
                amount: apply_percent(input.settlement_price, input.quantity.abs(), percent)?,
                percent,
                underlying: None,
            })
        }
        ContractKind::Call | ContractKind::Put if !input.quantity.is_sign_negative() => {
            Ok(ExtremeLoss {
                amount: Decimal::ZERO,
                percent: Decimal::ZERO,
                underlying: input.underlying,
            })
        }
        ContractKind::Call | ContractKind::Put => {
            let underlying = input.underlying.ok_or_else(|| {
                format!(
                    "SPAN {} has no underlying price in this file, so extreme loss margin is not calculated",
                    input.product
                )
            })?;
            if !underlying.is_sign_positive() {
                return Err(format!(
                    "SPAN {} underlying price is not positive, so extreme loss margin is not calculated",
                    input.product
                ));
            }
            let strike = input.strike.ok_or_else(|| {
                format!(
                    "SPAN {} short option has no strike, so extreme loss margin is not calculated",
                    input.product
                )
            })?;
            let expiry = input.expiry.ok_or_else(|| {
                format!(
                    "SPAN {} short option has no expiry, so extreme loss margin is not calculated",
                    input.product
                )
            })?;
            let percent = contract_option_percent(&input, strike, underlying, expiry)?;
            Ok(ExtremeLoss {
                amount: apply_percent(underlying, input.quantity.abs(), percent)?,
                percent,
                underlying: Some(underlying),
            })
        }
    }
}

fn apply_percent(price: Decimal, quantity: Decimal, percent: Decimal) -> Result<Decimal, String> {
    price
        .checked_mul(quantity)
        .and_then(|notional| notional.checked_mul(percent))
        .and_then(|scaled| scaled.checked_div(Decimal::from(100)))
        .ok_or_else(|| "extreme loss margin overflowed".to_owned())
}

fn short_option_percent(rate: ShortOptionRate<'_>) -> Result<Decimal, String> {
    if let Some(stock) = rate.exposure.stocks.get(rate.product) {
        let deep = deep_otm(rate.kind, rate.strike, rate.underlying, 30)?;
        return Ok(if deep {
            stock.otm_percent
        } else {
            stock.oth_percent
        });
    }
    let deep = deep_otm(rate.kind, rate.strike, rate.underlying, 10)?;
    let horizon = plus_months(rate.trading_date, 9)?;
    let long = rate.expiry > horizon;
    if deep && long {
        return Err(format!(
            "{} short option is more than 10 percent out of the money and expires more than nine months after {}, so neither published index rate is applied alone",
            rate.product, rate.trading_date
        ));
    }
    let mut percent = Decimal::from(if deep {
        INDEX_DEEP_OTM_PERCENT
    } else if long {
        INDEX_LONG_PERCENT
    } else {
        INDEX_ELM_PERCENT
    });
    if rate.expiry == rate.trading_date {
        percent += Decimal::from(INDEX_EXPIRY_EXTRA_PERCENT);
    }
    Ok(percent)
}

struct ShortOptionRate<'a> {
    exposure: &'a ExposureRates,
    product: &'a str,
    kind: ContractKind,
    strike: Decimal,
    underlying: Decimal,
    expiry: NaiveDate,
    trading_date: NaiveDate,
}

fn option_premium(
    kind: ContractKind,
    quantity: Decimal,
    price: Decimal,
) -> Result<Decimal, String> {
    if !kind.is_option() {
        return Ok(Decimal::ZERO);
    }
    quantity
        .checked_mul(price)
        .ok_or_else(|| "option premium overflowed".to_owned())
}

fn deep_otm(
    kind: ContractKind,
    strike: Decimal,
    underlying: Decimal,
    percent: u32,
) -> Result<bool, String> {
    let out_of_the_money = match kind {
        ContractKind::Call => strike > underlying,
        ContractKind::Put => strike < underlying,
        _ => false,
    };
    if !out_of_the_money {
        return Ok(false);
    }
    let gap = (strike - underlying).abs();
    let limit = underlying
        .checked_mul(Decimal::from(percent))
        .and_then(|value| value.checked_div(Decimal::from(100)))
        .ok_or_else(|| "out-of-the-money distance overflowed".to_owned())?;
    Ok(gap > limit)
}

fn plus_months(date: NaiveDate, months: u32) -> Result<NaiveDate, String> {
    let month_index = date.month0() + months;
    let year = date
        .year()
        .checked_add(i32::try_from(month_index / 12).unwrap_or(i32::MAX))
        .ok_or_else(|| "residual maturity could not be measured".to_owned())?;
    let month = (month_index % 12) + 1;
    if let Some(exact) = NaiveDate::from_ymd_opt(year, month, date.day()) {
        return Ok(exact);
    }
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year.checked_add(1).unwrap_or(year), 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next.and_then(|first| first.pred_opt())
        .ok_or_else(|| "residual maturity could not be measured".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        CashMarginRates, ContractElmRates, ExposureRates, MarginPosition, SpanMarginQuote,
        quote_span_xml,
    };
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 28).expect("date")
    }

    fn position(symbol: &str, quantity: &str) -> MarginPosition {
        MarginPosition {
            symbol: symbol.to_owned(),
            quantity: Decimal::from_str_exact(quantity).expect("qty"),
        }
    }

    fn amount(text: &str) -> Decimal {
        Decimal::from_str_exact(text).expect("amount")
    }

    fn exposure() -> ExposureRates {
        ExposureRates::parse_csv(
            b"Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date 28-Oct-2026,Total applicable ELM%\n1,RELIANCE,OTH,3.5,0,3.5\n2,RELIANCE,OTM,5.25,0,5.25\n",
            date(),
        )
        .expect("exposure")
    }

    fn value_book(xml: &[u8], positions: &[MarginPosition]) -> Result<SpanMarginQuote, String> {
        quote_span_xml(
            xml,
            date(),
            positions,
            &exposure(),
            &ContractElmRates::default(),
            &CashMarginRates::default(),
        )
    }

    fn document() -> String {
        r#"<?xml version="1.0"?>
<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<futPf><pfCode>NIFTY</pfCode>
<fut><pe>20261123</pe><p>22807.60</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>8.56</a><a>-2113.07</a><a>2130.20</a></ra>
</fut>
<fut><pe>20261229</pe><p>22951.50</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>1</a><a>2</a><a>3</a></ra>
</fut>
</futPf>
<ccDef><cc>NIFTY</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth></ccDef>
</spanFile>"#
            .to_owned()
    }

    fn book() -> String {
        r#"<?xml version="1.0"?>
<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<futPf><pfCode>NIFTY</pfCode>
<fut><pe>20261123</pe><p>22807.60</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>8.56</a><a>-2113.07</a><a>2130.20</a></ra>
</fut>
</futPf>
<oopPf><pfCode>NIFTY</pfCode>
<series><pe>20261103</pe><cvf>1.00</cvf><sc>1.00000</sc>
<opt><o>C</o><k>26000</k><p>10</p>
<ra><r>1</r><a>1</a><a>4</a><a>-2</a></ra>
</opt>
</series>
<series><pe>20261123</pe><cvf>1.00</cvf><sc>1.00000</sc>
<opt><o>C</o><k>26000</k><p>50</p>
<ra><r>1</r><a>0</a><a>-5.79</a><a>3</a></ra>
</opt>
<opt><o>P</o><k>26000</k><p>40</p>
<ra><r>1</r><a>2</a><a>0</a><a>-1</a></ra>
</opt>
</series>
</oopPf>
<phyPf><pfCode>NIFTY</pfCode>
<phy><pe>00000000</pe><p>22706.50</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>0</a><a>0</a><a>0</a></ra>
</phy>
</phyPf>
<futPf><pfCode>RELIANCE</pfCode>
<fut><pe>20261123</pe><p>1171</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>1</a><a>2</a><a>3</a></ra>
</fut>
<fut><pe>20261126</pe><p>1172</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>0.44</a><a>-55.05</a><a>166.92</a></ra>
</fut>
</futPf>
<oopPf><pfCode>RELIANCE</pfCode>
<series><pe>20261126</pe><cvf>1</cvf><sc>1</sc>
<opt><o>C</o><k>1400</k><p>12.5</p>
<ra><r>1</r><a>0.31</a><a>-54.85</a><a>55.48</a></ra>
</opt>
</series>
</oopPf>
<phyPf><pfCode>RELIANCE</pfCode>
<phy><pe>00000000</pe><p>1166.90</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>0.00</a><a>0.00</a><a>-0.00</a></ra>
</phy>
</phyPf>
<ccDef><cc>NIFTY</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0</val></rate></tier></somTiers>
</ccDef>
<ccDef><cc>RELIANCE</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0.00</val></rate></tier></somTiers>
</ccDef>
</spanFile>"#
            .to_owned()
    }

    #[test]
    fn short_november_future_uses_the_greatest_loss() {
        let quote =
            value_book(document().as_bytes(), &[position("NIFTY26NOVFUT", "-65")]).expect("quote");
        assert_eq!(quote.span_margin, amount("137349.55"));
        assert_eq!(quote.scanning_risk, quote.span_margin);
        assert_eq!(quote.spread_charge, Decimal::ZERO);
        assert_eq!(quote.short_option_minimum, Decimal::ZERO);
        assert_eq!(quote.extreme_loss_margin, amount("29649.88"));
        assert_eq!(quote.lines[0].elm_percent, amount("2"));
        assert_eq!(quote.premium, Decimal::ZERO);
        assert_eq!(quote.premium_margin, Decimal::ZERO);
        assert_eq!(quote.total_margin, amount("166999.43"));
        assert_eq!(quote.lines[0].instrument, "future");
        assert_eq!(quote.lines[0].strike, None);
        assert_eq!(quote.lines[0].worst_scan_points, vec![2]);
        assert_eq!(
            quote.lines[0].expiry,
            Some(NaiveDate::from_ymd_opt(2026, 11, 23).expect("expiry"))
        );
        assert_eq!(quote.lines[0].settlement_price, amount("22807.60"));
    }

    #[test]
    fn a_long_future_uses_its_greatest_loss_and_a_missing_contract_fails() {
        let quote =
            value_book(document().as_bytes(), &[position("NIFTY26NOVFUT", "65")]).expect("quote");
        assert_eq!(quote.span_margin, amount("138463"));
        assert_eq!(quote.extreme_loss_margin, amount("29649.88"));
        assert_eq!(quote.total_margin, amount("168112.88"));
        assert!(value_book(document().as_bytes(), &[position("NIFTY26OCTFUT", "1")]).is_err());
        let two = document().replace("20261229", "20261124");
        assert!(value_book(two.as_bytes(), &[position("NIFTY26NOVFUT", "1")]).is_err());
    }

    #[test]
    fn options_stocks_and_dated_stock_futures_use_their_risk_arrays() {
        let monthly = value_book(
            book().as_bytes(),
            &[
                position("NIFTY26NOV26000CE", "-1"),
                position("NIFTY26NOV26000PE", "1"),
            ],
        )
        .expect("same expiry");
        assert_eq!(monthly.lines[0].instrument, "call");
        assert_eq!(monthly.lines[0].scanning_risk, amount("5.79"));
        assert_eq!(monthly.lines[0].extreme_loss_margin, amount("681.20"));
        assert_eq!(monthly.lines[0].elm_percent, amount("3"));
        assert_eq!(monthly.lines[0].premium, amount("-50"));
        assert_eq!(monthly.lines[1].instrument, "put");
        assert_eq!(monthly.lines[1].extreme_loss_margin, Decimal::ZERO);
        assert_eq!(monthly.lines[1].premium, amount("40"));
        assert_eq!(monthly.premium, amount("-10"));
        assert_eq!(monthly.premium_margin, Decimal::ZERO);
        assert_eq!(monthly.span_margin, amount("5.79"));
        assert_eq!(monthly.total_margin, amount("696.99"));
        assert!(monthly.note.contains("short-option minimum rate"));
        assert_eq!(
            monthly.lines[0].expiry,
            Some(NaiveDate::from_ymd_opt(2026, 11, 23).expect("monthly"))
        );
        assert_eq!(monthly.lines[1].scanning_risk, amount("2"));

        let weekly =
            value_book(book().as_bytes(), &[position("NIFTY26N0326000CE", "-1")]).expect("weekly");
        assert_eq!(weekly.lines[0].scanning_risk, amount("2"));
        assert_eq!(weekly.lines[0].extreme_loss_margin, amount("681.20"));
        assert_eq!(
            weekly.lines[0].expiry,
            Some(NaiveDate::from_ymd_opt(2026, 11, 3).expect("weekly"))
        );

        let stock = value_book(book().as_bytes(), &[position("RELIANCE", "5")]).expect("stock");
        assert_eq!(stock.lines[0].instrument, "stock");
        assert_eq!(stock.lines[0].expiry, None);
        assert_eq!(stock.lines[0].scanning_risk, Decimal::ZERO);
        assert_eq!(stock.lines[0].extreme_loss_margin, Decimal::ZERO);
        assert_eq!(stock.lines[0].settlement_price, amount("1166.90"));
        assert_eq!(stock.total_margin, Decimal::ZERO);

        let future = value_book(book().as_bytes(), &[position("RELIANCE26N26FUT", "-1")])
            .expect("dated future");
        assert_eq!(future.lines[0].instrument, "future");
        assert_eq!(future.lines[0].scanning_risk, amount("55.05"));
        assert_eq!(future.lines[0].extreme_loss_margin, amount("41.02"));
        assert_eq!(future.lines[0].elm_percent, amount("3.5"));
        assert_eq!(future.total_margin, amount("96.07"));
        assert_eq!(
            future.lines[0].expiry,
            Some(NaiveDate::from_ymd_opt(2026, 11, 26).expect("day"))
        );

        let option = value_book(book().as_bytes(), &[position("RELIANCE26N261400CE", "-2")])
            .expect("stock option");
        assert_eq!(option.lines[0].instrument, "call");
        assert_eq!(option.lines[0].strike, Some(amount("1400")));
        assert_eq!(option.lines[0].scanning_risk, amount("109.70"));
        assert_eq!(option.lines[0].underlying_price, Some(amount("1166.90")));
        assert_eq!(option.lines[0].elm_percent, amount("3.5"));
        assert_eq!(option.lines[0].extreme_loss_margin, amount("81.68"));
        assert_eq!(option.lines[0].premium, amount("-25"));
        assert_eq!(option.premium_margin, Decimal::ZERO);
        assert_eq!(option.total_margin, amount("216.38"));

        let ambiguous = value_book(book().as_bytes(), &[position("RELIANCE26NOVFUT", "1")])
            .expect_err("two November futures");
        assert!(ambiguous.contains("RELIANCE26N23FUT"), "{ambiguous}");
        assert!(ambiguous.contains("RELIANCE26N26FUT"), "{ambiguous}");
    }

    #[test]
    fn a_monthly_option_without_one_futures_expiry_is_rejected() {
        let xml = book().replacen("<fut><pe>20261123</pe>", "<fut><pe>20261223</pe>", 1);
        let error = value_book(xml.as_bytes(), &[position("NIFTY26NOV26000CE", "-1")])
            .expect_err("ambiguous month");
        assert!(error.contains("NIFTY26N0326000CE"));
        assert!(error.contains("NIFTY26N2326000CE"));
        let priced = book().replace("<val>0</val>", "<val>5</val>");
        assert!(value_book(priced.as_bytes(), &[position("NIFTY26NOV26000CE", "-1")]).is_err());
        let long = value_book(priced.as_bytes(), &[position("NIFTY26NOV26000CE", "1")])
            .expect("long option");
        assert_eq!(long.short_option_minimum, Decimal::ZERO);
        assert_eq!(long.lines[0].scanning_risk, amount("3"));
        assert_eq!(long.lines[0].extreme_loss_margin, Decimal::ZERO);
        assert_eq!(long.premium, amount("50"));
        assert_eq!(long.premium_margin, amount("50"));
        assert_eq!(long.total_margin, Decimal::ZERO);
    }

    #[test]
    fn index_short_option_rates_follow_the_published_schedule() {
        let expiry_day = book()
            .replace("<pe>20261103</pe>", "<pe>20261028</pe>")
            .replace("<k>26000</k><p>10</p>", "<k>22000</k><p>10</p>");
        let on_expiry = value_book(
            expiry_day.as_bytes(),
            &[position("NIFTY26O2822000CE", "-1")],
        )
        .expect("expiry day");
        assert_eq!(on_expiry.lines[0].elm_percent, amount("4"));
        assert_eq!(on_expiry.extreme_loss_margin, amount("908.26"));

        let far = book()
            .replace("<pe>20261103</pe>", "<pe>20270729</pe>")
            .replace("<k>26000</k><p>10</p>", "<k>22000</k><p>10</p>");
        let long_dated =
            value_book(far.as_bytes(), &[position("NIFTY2772922000CE", "-1")]).expect("far");
        assert_eq!(long_dated.lines[0].elm_percent, amount("5"));

        let both = book()
            .replace("<pe>20261103</pe>", "<pe>20270729</pe>")
            .replace("<k>26000</k><p>10</p>", "<k>30000</k><p>10</p>");
        let error = value_book(both.as_bytes(), &[position("NIFTY2772930000CE", "-1")])
            .expect_err("two index rates");
        assert!(error.contains("neither published index rate"), "{error}");
    }

    #[test]
    fn stock_extreme_loss_uses_the_exposure_file_total() {
        let deep = book().replace("<k>1400</k>", "<k>2000</k>");
        let option =
            value_book(deep.as_bytes(), &[position("RELIANCE26N262000CE", "-1")]).expect("deep");
        assert_eq!(option.lines[0].elm_percent, amount("5.25"));
        assert_eq!(option.extreme_loss_margin, amount("61.26"));

        let raised = ExposureRates::parse_csv(
            b"Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date 28-Oct-2026,Total applicable ELM%\n1,RELIANCE,OTH,3.5,15,18.5\n2,RELIANCE,OTM,5.25,15,20.25\n",
            date(),
        )
        .expect("raised");
        let future = quote_span_xml(
            book().as_bytes(),
            date(),
            &[position("RELIANCE26N26FUT", "-1")],
            &raised,
            &ContractElmRates::default(),
            &CashMarginRates::default(),
        )
        .expect("additional");
        assert_eq!(future.lines[0].elm_percent, amount("18.5"));
        assert_eq!(future.extreme_loss_margin, amount("216.82"));

        let mismatch = ExposureRates::parse_csv(
            b"Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date 28-Oct-2026,Total applicable ELM%\n1,RELIANCE,OTH,3.5,0,4\n2,RELIANCE,OTM,5.25,0,5.25\n",
            date(),
        );
        assert!(mismatch.is_err());
    }

    #[test]
    fn a_short_option_without_an_underlying_price_is_rejected() {
        let xml = book().replace(
            "<phyPf><pfCode>NIFTY</pfCode>\n<phy><pe>00000000</pe><p>22706.50</p><cvf>1</cvf><sc>1</sc>\n<ra><r>1</r><a>0</a><a>0</a><a>0</a></ra>\n</phy>\n</phyPf>\n",
            "",
        );
        let error =
            value_book(xml.as_bytes(), &[position("NIFTY26N0326000CE", "-1")]).expect_err("price");
        assert!(error.contains("underlying price"), "{error}");
    }

    fn spread_file(near_expiry: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<futPf><pfCode>NIFTY</pfCode>
<fut><pe>{near_expiry}</pe><p>100</p><d>1</d><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>10</a><a>-4</a></ra></fut>
<fut><pe>20261229</pe><p>110</p><d>1</d><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>20</a><a>-8</a></ra></fut>
</futPf>
<ccDef><cc>NIFTY</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0</val></rate></tier></somTiers>
</ccDef>
<dSpread>
<spread>1</spread><chargeMeth>F</chargeMeth><rate><r>1</r><val>5</val></rate>
<pLeg><cc>NIFTY</cc><pe>{near_expiry}</pe><rs>A</rs><i>1</i></pLeg>
<pLeg><cc>NIFTY</cc><pe>20261229</pe><rs>B</rs><i>1</i></pLeg>
</dSpread>
<dSpread><spread></spread><chargeMeth></chargeMeth><rate><r></r><val></val></rate></dSpread>
</spanFile>"#
        )
    }

    #[test]
    fn a_futures_calendar_spread_charges_the_file_and_one_third_of_the_far_month() {
        let positions = [
            position("NIFTY26NOVFUT", "2"),
            position("NIFTY26DECFUT", "-2"),
        ];
        let quote = value_book(spread_file("20261123").as_bytes(), &positions).expect("spread");
        assert_eq!(quote.scanning_risk, amount("8"));
        assert_eq!(quote.spread_charge, amount("10"));
        assert_eq!(quote.span_margin, amount("18"));
        assert_eq!(quote.lines[0].extreme_loss_margin, Decimal::ZERO);
        assert_eq!(quote.lines[1].extreme_loss_margin, amount("1.47"));
        assert_eq!(quote.lines[1].elm_percent, amount("2"));
        assert_eq!(quote.delivery_margin, Decimal::ZERO);
        assert_eq!(quote.cross_margin_benefit, Decimal::ZERO);
        assert_eq!(quote.lines[0].total_margin, amount("24"));
        assert_eq!(quote.lines[1].total_margin, amount("20.40"));
        assert_eq!(quote.margin_benefit, amount("24.93"));
        assert_eq!(quote.total_margin, amount("19.47"));

        let held = value_book(
            book().as_bytes(),
            &[
                position("RELIANCE", "5"),
                position("RELIANCE26N26FUT", "-1"),
            ],
        )
        .expect("stock and future");
        assert_eq!(held.scanning_risk, amount("55.05"));
        assert_eq!(held.spread_charge, Decimal::ZERO);
        assert_eq!(held.total_margin, amount("96.07"));
    }

    #[test]
    fn an_index_spread_does_not_offset_a_contract_expiring_today() {
        let positions = [
            position("NIFTY26OCTFUT", "2"),
            position("NIFTY26DECFUT", "-2"),
        ];
        let quote = value_book(spread_file("20261028").as_bytes(), &positions).expect("expiry");
        assert_eq!(quote.scanning_risk, amount("36"));
        assert_eq!(quote.spread_charge, Decimal::ZERO);
        assert_eq!(quote.extreme_loss_margin, amount("8.40"));
        assert_eq!(quote.span_margin, amount("36"));
    }

    #[test]
    fn the_contract_file_supplies_the_extreme_loss_percent() {
        let rates = ContractElmRates::parse_csv(
            b"FUTIDX,NIFTY,23-NOV-2026,-1,XX,0,2.5\nOPTIDX,NIFTY,03-NOV-2026,26000,CE,0,3\n",
        )
        .expect("contracts");
        let quote = quote_span_xml(
            document().as_bytes(),
            date(),
            &[position("NIFTY26NOVFUT", "-65")],
            &exposure(),
            &rates,
            &CashMarginRates::default(),
        )
        .expect("file rate");
        assert_eq!(quote.lines[0].canonical, "NSE:NIFTY:2026-11-23:FUT");
        assert_eq!(quote.lines[0].elm_percent, amount("2.5"));
        assert_eq!(quote.extreme_loss_margin, amount("37062.35"));
        let missing = quote_span_xml(
            document().as_bytes(),
            date(),
            &[position("NIFTY26DECFUT", "1")],
            &exposure(),
            &rates,
            &CashMarginRates::default(),
        )
        .expect_err("missing contract");
        assert!(missing.contains("not in the contract file"), "{missing}");
    }

    #[test]
    fn delivery_margin_uses_the_cash_rate_on_the_net_deliverable_shares() {
        let outside =
            value_book(book().as_bytes(), &[position("RELIANCE26N26FUT", "-1")]).expect("outside");
        assert_eq!(outside.delivery_margin, Decimal::ZERO);
        assert_eq!(outside.cross_margin_benefit, Decimal::ZERO);
        let inside = book().replace("<pe>20261126</pe>", "<pe>20261103</pe>");
        let missing = value_book(inside.as_bytes(), &[position("RELIANCE26N03FUT", "-1")])
            .expect_err("inside window");
        assert!(missing.contains("capital-market VaR"), "{missing}");

        let cash = CashMarginRates::parse(
            b"10,28102026,0.00,06,0000001\n20,RELIANCE,EQ,INE002A01018,8.55,0.00,9.00,3.50,0.00,12.50\n",
            date(),
        )
        .expect("var");
        let quote = quote_span_xml(
            inside.as_bytes(),
            date(),
            &[position("RELIANCE26N03FUT", "-1")],
            &exposure(),
            &ContractElmRates::default(),
            &cash,
        )
        .expect("delivery");
        assert_eq!(quote.delivery_margin, amount("14.59"));
        assert_eq!(quote.scanning_risk, amount("55.05"));
        assert_eq!(quote.extreme_loss_margin, amount("41.02"));
        assert_eq!(quote.total_margin, amount("110.66"));

        let otm =
            value_book(inside.as_bytes(), &[position("RELIANCE26N031400CE", "-2")]).expect("otm");
        assert_eq!(otm.delivery_margin, Decimal::ZERO);

        let offset = inside.replace("<o>C</o><k>1400</k>", "<o>P</o><k>1400</k>");
        let netted = value_book(
            offset.as_bytes(),
            &[
                position("RELIANCE26N03FUT", "1"),
                position("RELIANCE26N031400PE", "1"),
            ],
        )
        .expect("netted");
        assert_eq!(netted.delivery_margin, Decimal::ZERO);

        let expiry_day = book().replace("<pe>20261126</pe>", "<pe>20261028</pe>");
        let expiry = quote_span_xml(
            expiry_day.as_bytes(),
            date(),
            &[position("RELIANCE26O28FUT", "-1")],
            &exposure(),
            &ContractElmRates::default(),
            &cash,
        )
        .expect("expiry day");
        assert_eq!(expiry.delivery_margin, amount("145.86"));

        let other = CashMarginRates::parse(
            b"10,28102026,0.00,06,0000001\n20,HDFCBANK,EQ,INE040A01034,8.39,0.00,9.00,3.50,0.00,12.50\n",
            date(),
        )
        .expect("other");
        let absent = quote_span_xml(
            inside.as_bytes(),
            date(),
            &[position("RELIANCE26N03FUT", "-1")],
            &exposure(),
            &ContractElmRates::default(),
            &other,
        )
        .expect_err("missing symbol");
        assert!(absent.contains("no EQ series"), "{absent}");
    }

    #[test]
    fn the_contract_file_identifies_the_expiry_and_a_canonical_symbol_selects_one() {
        let rates = ContractElmRates::parse_csv(
            b"FUTSTK,RELIANCE,23-NOV-2026,-1,XX,0,3.5\nFUTSTK,RELIANCE,26-NOV-2026,-1,XX,0,3.5\nFUTIDX,NIFTY,23-NOV-2026,-1,XX,0,2\nOPTIDX,NIFTY,03-NOV-2026,26000,CE,0,3\n",
        )
        .expect("contracts");
        let ambiguous = quote_span_xml(
            book().as_bytes(),
            date(),
            &[position("RELIANCE26NOVFUT", "1")],
            &exposure(),
            &rates,
            &CashMarginRates::default(),
        )
        .expect_err("two expiries");
        assert!(
            ambiguous.contains("NSE:RELIANCE:2026-11-23:FUT"),
            "{ambiguous}"
        );
        assert!(
            ambiguous.contains("NSE:RELIANCE:2026-11-26:FUT"),
            "{ambiguous}"
        );

        let chosen = quote_span_xml(
            book().as_bytes(),
            date(),
            &[position("NSE:RELIANCE:2026-11-26:FUT", "-1")],
            &exposure(),
            &rates,
            &CashMarginRates::default(),
        )
        .expect("canonical");
        assert_eq!(chosen.lines[0].canonical, "NSE:RELIANCE:2026-11-26:FUT");
        assert_eq!(
            chosen.lines[0].expiry,
            Some(NaiveDate::from_ymd_opt(2026, 11, 26).expect("expiry"))
        );

        let only_far = ContractElmRates::parse_csv(b"FUTSTK,RELIANCE,26-NOV-2026,-1,XX,0,3.5\n")
            .expect("one expiry");
        let resolved = quote_span_xml(
            book().as_bytes(),
            date(),
            &[position("RELIANCE26NOVFUT", "-1")],
            &exposure(),
            &only_far,
            &CashMarginRates::default(),
        )
        .expect("identified");
        assert_eq!(resolved.lines[0].canonical, "NSE:RELIANCE:2026-11-26:FUT");

        let index =
            value_book(document().as_bytes(), &[position("NIFTY26NOVFUT", "-65")]).expect("index");
        assert_eq!(index.lines[0].canonical, "NSE:NIFTY:2026-11-23:FUT");
        let stock = value_book(book().as_bytes(), &[position("RELIANCE", "5")]).expect("stock");
        assert_eq!(stock.lines[0].canonical, "NSE:RELIANCE:EQ");
        let call = quote_span_xml(
            book().as_bytes(),
            date(),
            &[position("NSE:NIFTY:2026-11-03:26000:CE", "-1")],
            &exposure(),
            &rates,
            &CashMarginRates::default(),
        )
        .expect("call");
        assert_eq!(call.lines[0].canonical, "NSE:NIFTY:2026-11-03:26000:CE");

        let wrong = quote_span_xml(
            book().as_bytes(),
            date(),
            &[position("FUTIDX:RELIANCE:2026-11-26", "-1")],
            &exposure(),
            &rates,
            &CashMarginRates::default(),
        )
        .expect_err("index prefix");
        assert!(wrong.contains("not in the contract file"), "{wrong}");

        let repeated = quote_span_xml(
            book().as_bytes(),
            date(),
            &[
                position("RELIANCE26N26FUT", "-1"),
                position("NSE:RELIANCE:2026-11-26:FUT", "1"),
            ],
            &exposure(),
            &rates,
            &CashMarginRates::default(),
        )
        .expect_err("same contract");
        assert!(repeated.contains("same contract"), "{repeated}");
    }
}
