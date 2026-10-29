//! Position symbols. A symbol is a monthly contract, a dated contract, a
//! canonical `EXCHANGE:PRODUCT:...` name, or a physical stock code.

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use std::collections::BTreeSet;
use std::str::FromStr;

pub(super) const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarginPosition {
    pub symbol: String,
    pub quantity: Decimal,
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
    if exchange_code != "NSE" && exchange_code != "BSE" {
        return Err("SPAN margin symbols use the NSE or BSE exchange".to_owned());
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
            Ok(format!("{exchange_code}:{product}:{expiry}:FUT"))
        }
        "EQ" => {
            if expiry.is_some_and(|expiry| !expiry.trim().is_empty())
                || strike_text.is_some_and(|strike| !strike.trim().is_empty())
            {
                return Err("a physical stock has no expiry or strike".to_owned());
            }
            Ok(format!("{exchange_code}:{product}:EQ"))
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
                "{exchange_code}:{product}:{expiry}:{}:{side}",
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

pub(super) fn parse_positions(positions: &[MarginPosition]) -> Result<Vec<SymbolSpec>, String> {
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

#[derive(Clone, Debug)]
pub(super) enum SymbolSpec {
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
    if matches!(parts.first(), Some(&"NSE" | &"BSE")) {
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
            "{symbol} is not a canonical symbol. Use NSE:PRODUCT:YYYY-MM-DD:FUT, BSE:PRODUCT:YYYY-MM-DD:FUT, NSE:PRODUCT:YYYY-MM-DD:STRIKE:CE, NSE:PRODUCT:YYYY-MM-DD:STRIKE:PE, or NSE:PRODUCT:EQ"
        )),
    }
}

fn parse_exchange_symbol(symbol: &str, parts: &[&str]) -> Result<SymbolSpec, String> {
    let exchange = parts.first().copied().unwrap_or("");
    match &parts[1..] {
        [product, "EQ"] => {
            if !ascii_product(product) {
                return Err(format!("{symbol} is not a canonical stock symbol"));
            }
            Ok(SymbolSpec::Stock {
                product: (*product).to_owned(),
            })
        }
        [product, date, "FUT"] => {
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
        [product, date, strike_text, right] => {
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
            "{symbol} is not a canonical symbol. Use {exchange}:PRODUCT:YYYY-MM-DD:FUT, {exchange}:PRODUCT:YYYY-MM-DD:STRIKE:CE, {exchange}:PRODUCT:YYYY-MM-DD:STRIKE:PE, or {exchange}:PRODUCT:EQ"
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

pub(super) fn product_year_month(body: &str) -> Option<(&str, i32, u32)> {
    if body.len() < 5 {
        return None;
    }
    let month_name = &body[body.len() - 3..];
    let year_text = &body[body.len() - 5..body.len() - 3];
    let product = &body[..body.len() - 5];
    let month = u32::try_from(MONTHS.iter().position(|month| *month == month_name)? + 1).ok()?;
    if !ascii_digits(year_text) {
        return None;
    }
    let year = 2000 + year_text.parse::<i32>().ok()?;
    Some((product, year, month))
}

fn monthly_contract(body: &str) -> Option<(String, i32, u32)> {
    let (product, year, month) = product_year_month(body)?;
    if product.is_empty() || !ascii_product(product) {
        return None;
    }
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

pub(super) fn strike(symbol: &str, text: &str) -> Result<Decimal, String> {
    let strike = Decimal::from_str(text)
        .map_err(|_| format!("{symbol} does not contain a decimal strike"))?;
    if strike.is_sign_negative() {
        return Err(format!("{symbol} has a negative strike"));
    }
    Ok(strike)
}

pub(super) fn day(symbol: &str, text: &str) -> Result<u32, String> {
    let day = text
        .parse::<u32>()
        .map_err(|_| format!("{symbol} does not contain a two-digit day"))?;
    if !(1..=31).contains(&day) {
        return Err(format!("{symbol} does not contain a calendar day"));
    }
    Ok(day)
}

pub(super) fn month_from_code(code: char) -> Option<u32> {
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

pub(super) fn week_code(month: u32) -> char {
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

pub(super) fn expiry_matches(
    expiry: Option<NaiveDate>,
    year: i32,
    month: u32,
    day: Option<u32>,
) -> bool {
    let Some(expiry) = expiry else {
        return false;
    };
    expiry.year() == year && expiry.month() == month && day.is_none_or(|day| expiry.day() == day)
}

impl SymbolSpec {
    pub(super) fn wants_future(&self, product: &str, expiry: NaiveDate) -> bool {
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

    pub(super) fn wants_option(
        &self,
        product: &str,
        call: bool,
        strike: Decimal,
        expiry: NaiveDate,
    ) -> bool {
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

    pub(super) fn wants_stock(&self, product: &str) -> bool {
        matches!(self, Self::Stock { product: name } if name == product)
    }

    pub(super) fn product(&self) -> &str {
        match self {
            Self::Future { product, .. }
            | Self::Option { product, .. }
            | Self::Stock { product } => product,
        }
    }
}
