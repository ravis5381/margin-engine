//! Exposure, contract extreme-loss, and cash-market VaR files that sit beside a SPAN archive.

use super::file::{ContractKind, UNCOMPRESSED_LIMIT, canonical_parts};
use super::symbol::{MarginPosition, SymbolSpec, expiry_matches, product_year_month};
use crate::zip_safe::{ZipLimits, extract_zip_to_memory};
use chrono::{Datelike, NaiveDate};
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::io::Read;
use std::str::FromStr;

pub(super) const INDEX_ELM_PERCENT: u32 = 2;
pub(super) const INDEX_DEEP_OTM_PERCENT: u32 = 3;
pub(super) const INDEX_LONG_PERCENT: u32 = 5;
pub(super) const INDEX_EXPIRY_EXTRA_PERCENT: u32 = 2;

#[derive(Clone, Debug)]
pub struct ExposureRates {
    stocks: BTreeMap<String, StockElm>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct StockElm {
    oth_percent: Decimal,
    otm_percent: Decimal,
}

impl ExposureRates {
    pub fn none() -> Self {
        Self {
            stocks: BTreeMap::new(),
        }
    }

    pub fn parse_csv(bytes: &[u8], trading_date: NaiveDate) -> Result<Self, String> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| "exposure file is not UTF-8".to_owned())?;
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default().trim_end_matches('\r');
        let expected = format!(
            "Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date {},Total applicable ELM%",
            crate::nse::reports::display_date(trading_date)
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

    pub(super) fn contains_stock(&self, product: &str) -> bool {
        self.stocks.contains_key(product)
    }

    pub(super) fn future_percent(&self, product: &str) -> Decimal {
        self.stocks
            .get(product)
            .map(|stock| stock.oth_percent)
            .unwrap_or_else(|| Decimal::from(INDEX_ELM_PERCENT))
    }

    pub(super) fn option_percents(&self, product: &str) -> Option<(Decimal, Decimal)> {
        self.stocks
            .get(product)
            .map(|stock| (stock.oth_percent, stock.otm_percent))
    }
}

pub(super) fn elm_percent(text: &str, row: usize, label: &str) -> Result<Decimal, String> {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContractElmRates {
    rows: BTreeMap<(String, NaiveDate, ContractKind, Option<Decimal>), Decimal>,
    listed: Vec<ListedContract>,
    monthly_futures: BTreeMap<(String, i32, u32), Decimal>,
    iccl: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListedContract {
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
        Ok(Self {
            rows,
            listed,
            monthly_futures: BTreeMap::new(),
            iccl: false,
        })
    }

    pub fn parse_iccl_zip(bytes: &[u8]) -> Result<Self, String> {
        let entries = extract_zip_to_memory(bytes, ZipLimits::conservative(UNCOMPRESSED_LIMIT))
            .map_err(|error| error.to_string())?;
        if entries.len() != 1 || !entries[0].name.to_ascii_lowercase().ends_with(".csv") {
            return Err("ICCL extreme loss zip does not contain one .csv member".to_owned());
        }
        Self::parse_iccl_csv(&entries[0].bytes)
    }

    pub(super) fn parse_iccl_csv(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "ICCL extreme loss file is not UTF-8".to_owned())?;
        let mut monthly_futures = BTreeMap::new();
        let mut rows_seen = 0_usize;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() != 6 {
                return Err(format!(
                    "ICCL extreme loss file row {} does not have 6 fields",
                    index + 1
                ));
            }
            if fields[0].trim().parse::<u64>().is_err() {
                return Err(format!(
                    "ICCL extreme loss file row {} token is not an integer",
                    index + 1
                ));
            }
            let normal = elm_percent(fields[3], index + 1, "normal")?;
            let additional = elm_percent(fields[4], index + 1, "additional")?;
            let total = elm_percent(fields[5], index + 1, "total")?;
            if normal
                .checked_add(additional)
                .is_none_or(|sum| sum != total)
            {
                return Err(format!(
                    "ICCL extreme loss total for {} is not the normal percent plus the additional percent",
                    fields[1].trim()
                ));
            }
            rows_seen += 1;
            match fields[2].trim() {
                "IF" | "SF" => {
                    let Some((product, year, month)) = iccl_future_symbol(fields[1].trim()) else {
                        return Err(format!(
                            "ICCL extreme loss future `{}` is not PRODUCTYYMONFUT",
                            fields[1].trim()
                        ));
                    };
                    let key = (product, year, month);
                    if let Some(existing) = monthly_futures.get(&key)
                        && existing != &total
                    {
                        return Err(format!(
                            "ICCL extreme loss file repeats {} with different percents",
                            fields[1].trim()
                        ));
                    }
                    monthly_futures.insert(key, total);
                }
                "IO" | "SO" => {}
                other => {
                    return Err(format!(
                        "ICCL extreme loss instrument `{other}` is not IF, SF, IO, or SO"
                    ));
                }
            }
        }
        if rows_seen == 0 {
            return Err("ICCL extreme loss file has no data row".to_owned());
        }
        Ok(Self {
            rows: BTreeMap::new(),
            listed: Vec::new(),
            monthly_futures,
            iccl: true,
        })
    }

    pub(super) fn monthly_future(&self, product: &str, expiry: NaiveDate) -> Option<Decimal> {
        self.monthly_futures
            .get(&(product.to_owned(), expiry.year(), expiry.month()))
            .copied()
    }

    pub fn parse_gzip(bytes: &[u8]) -> Result<Self, String> {
        let mut decoder = GzDecoder::new(bytes);
        let mut csv = Vec::new();
        decoder
            .read_to_end(&mut csv)
            .map_err(|_| "contract ELM file is not valid gzip".to_owned())?;
        Self::parse_csv(&csv)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(super) fn percent(
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

    pub(super) fn identify(
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
    pub(super) fn matches(&self, spec: &SymbolSpec) -> bool {
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

    pub(super) fn canonical(&self) -> String {
        canonical_parts("NSE", &self.product, self.expiry, self.strike, self.kind)
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

    pub(super) fn daily_percent(&self, product: &str) -> Result<Decimal, String> {
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

pub(super) fn contract_strike(
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

fn iccl_future_symbol(symbol: &str) -> Option<(String, i32, u32)> {
    let body = symbol.strip_suffix("FUT")?;
    let (product, year, month) = product_year_month(body)?;
    if !iccl_product(product) {
        return None;
    }
    Some((product.to_owned(), year, month))
}

fn iccl_product(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '&'
        })
}

pub(super) fn published_or_contract(
    contracts: &ContractElmRates,
    product: &str,
    kind: ContractKind,
    expiry: Option<NaiveDate>,
    strike: Option<Decimal>,
    published: Decimal,
) -> Result<Decimal, String> {
    if kind == ContractKind::Future
        && let Some(expiry) = expiry
        && let Some(percent) = contracts.monthly_future(product, expiry)
    {
        return Ok(percent);
    }
    if contracts.iccl && kind == ContractKind::Future {
        let when = expiry
            .map(|expiry| expiry.to_string())
            .unwrap_or_else(|| "with no expiry".to_owned());
        return Err(format!(
            "ICCL extreme loss file has no future rate for {product} {when}"
        ));
    }
    if contracts.is_empty() {
        return Ok(published);
    }
    let expiry = expiry.ok_or_else(|| {
        format!("SPAN {product} has no expiry, so the contract ELM file cannot be read")
    })?;
    contracts.percent(product, kind, expiry, strike)
}
