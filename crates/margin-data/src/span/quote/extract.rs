//! Walk of a SPAN 4.00 XML file. A quote keeps the contracts it asked for.
//! An index keeps every contract so the next quote does not walk the XML.

use super::file::{Book, CalendarSpread, Commodity, ContractKind, SpanContract};
use super::symbol::SymbolSpec;
use chrono::NaiveDate;
use quick_xml::Reader;
use quick_xml::events::Event;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tag {
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
    Exch,
    PriceMeth,
    Name,
    PfLink,
    PfType,
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

pub(super) fn classify(name: &[u8]) -> Tag {
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
        b"exch" => Tag::Exch,
        b"priceMeth" => Tag::PriceMeth,
        b"name" => Tag::Name,
        b"pfLink" => Tag::PfLink,
        b"pfType" => Tag::PfType,
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

pub(super) struct ExtractState<'a> {
    interest: &'a [SymbolSpec],
    aliases: BTreeMap<String, String>,
    retain_all: bool,
    spread_faults: Vec<SpreadFault>,
    exchange: Option<String>,
    indexes: BTreeSet<String>,
    price_method: Option<String>,
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

#[derive(Clone)]
pub(crate) struct CommodityFamily {
    pub cc: String,
    pub name: String,
    pub portfolios: Vec<(String, String)>,
}

#[derive(Clone)]
pub(crate) struct SpreadFault {
    pub products: Vec<String>,
    pub detail: String,
}

pub(crate) struct SpanFileIndex {
    pub business_date: NaiveDate,
    pub exchange: String,
    pub families: Vec<CommodityFamily>,
    pub contracts: Vec<SpanContract>,
    pub commodities: BTreeMap<String, Commodity>,
    pub underlyings: BTreeMap<String, Decimal>,
    pub spreads: BTreeMap<String, Vec<CalendarSpread>>,
    pub indexes: BTreeSet<String>,
    pub spread_faults: Vec<SpreadFault>,
}

impl SpanFileIndex {
    pub(super) fn book_for(
        &self,
        trading_date: NaiveDate,
        interest: &[SymbolSpec],
    ) -> Result<Book, String> {
        if self.business_date != trading_date {
            return Err(format!(
                "SPAN business date {} does not match trading date {trading_date}",
                self.business_date
            ));
        }
        let aliases = aliases_for(&self.families, interest)?;
        let name_of = |code: &str| -> String {
            aliases
                .get(code)
                .cloned()
                .unwrap_or_else(|| code.to_owned())
        };
        for fault in &self.spread_faults {
            if fault
                .products
                .iter()
                .any(|code| product_wanted(interest, &name_of(code)))
            {
                return Err(fault.detail.clone());
            }
        }
        let mut contracts = Vec::new();
        for contract in &self.contracts {
            let product = name_of(&contract.product);
            let keep = requested(
                interest,
                contract.kind,
                &product,
                contract.expiry,
                contract.strike,
            );
            if !keep {
                continue;
            }
            if listed_twice(
                &contracts,
                contract.kind,
                &product,
                contract.expiry,
                contract.strike,
            ) {
                return Err(format!(
                    "SPAN {product} {} is repeated",
                    described(contract.kind, contract.expiry, contract.strike)
                ));
            }
            let mut kept = contract.clone();
            kept.product = product;
            contracts.push(kept);
        }
        let mut commodities = BTreeMap::new();
        for (code, commodity) in &self.commodities {
            let name = name_of(code);
            if commodities
                .insert(name.clone(), commodity.clone())
                .is_some()
            {
                return Err(format!("SPAN combined commodity {name} is repeated"));
            }
        }
        let mut underlyings = BTreeMap::new();
        for (code, price) in &self.underlyings {
            let product = name_of(code);
            if !product_wanted(interest, &product) {
                continue;
            }
            if underlyings.insert(product.clone(), *price).is_some() {
                return Err(format!(
                    "SPAN physical {product} underlying price is repeated"
                ));
            }
        }
        let mut spreads = BTreeMap::new();
        for (code, file_spreads) in &self.spreads {
            let product = name_of(code);
            if !product_wanted(interest, &product) {
                continue;
            }
            let kept = spreads.entry(product.clone()).or_insert_with(Vec::new);
            for spread in file_spreads {
                if kept
                    .iter()
                    .any(|existing: &CalendarSpread| existing.number == spread.number)
                {
                    return Err(format!(
                        "SPAN calendar spread {} for {product} is repeated",
                        spread.number
                    ));
                }
                kept.push(spread.clone());
            }
            kept.sort_by_key(|spread| spread.number);
        }
        let mut indexes = BTreeSet::new();
        for code in &self.indexes {
            indexes.insert(name_of(code));
        }
        Ok(Book {
            exchange: self.exchange.clone(),
            contracts,
            commodities,
            underlyings,
            spreads,
            indexes,
        })
    }
}

fn portfolio_aliases(
    bytes: &[u8],
    interest: &[SymbolSpec],
) -> Result<BTreeMap<String, String>, String> {
    let families = combined_commodities(bytes)?;
    aliases_for(&families, interest)
}

fn aliases_for(
    families: &[CommodityFamily],
    interest: &[SymbolSpec],
) -> Result<BTreeMap<String, String>, String> {
    let wanted = interest
        .iter()
        .map(SymbolSpec::product)
        .filter(|product| *product == "SENSEX" || *product == "BSX")
        .collect::<Vec<_>>();
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut aliases = BTreeMap::new();
    for product in wanted {
        let family = if product == "BSX" {
            families.iter().find(|family| family.cc == "BSX")
        } else {
            families
                .iter()
                .find(|family| family.name == "BSE 30 SENSEX")
        };
        let Some(family) = family else {
            return Err(if product == "SENSEX" {
                "SPAN file has no combined commodity named BSE 30 SENSEX".to_owned()
            } else {
                "SPAN file has no combined commodity BSX".to_owned()
            });
        };
        if !family
            .portfolios
            .iter()
            .any(|(_, kind)| kind == "FUT" || kind == "PHY" || kind == "OOP")
        {
            return Err(format!(
                "SPAN combined commodity {} has no portfolio link",
                family.cc
            ));
        }
        bind_alias(&mut aliases, &family.cc, product)?;
        for (code, _) in &family.portfolios {
            bind_alias(&mut aliases, code, product)?;
        }
    }
    Ok(aliases)
}

fn bind_alias(
    aliases: &mut BTreeMap<String, String>,
    code: &str,
    product: &str,
) -> Result<(), String> {
    if let Some(existing) = aliases.insert(code.to_owned(), product.to_owned())
        && existing != product
    {
        return Err(format!(
            "SPAN portfolio {code} belongs to more than one requested product"
        ));
    }
    Ok(())
}

fn combined_commodities(bytes: &[u8]) -> Result<Vec<CommodityFamily>, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack: Vec<Tag> = Vec::new();
    let mut families = Vec::new();
    let mut cc = None;
    let mut name = None;
    let mut portfolios = Vec::new();
    let mut link_code = None;
    let mut link_kind = None;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("SPAN XML could not be read: {error}"))?;
        match event {
            Event::Start(element) => {
                let tag = classify(element.local_name().as_ref());
                if tag == Tag::CcDef {
                    cc = None;
                    name = None;
                    portfolios.clear();
                }
                if tag == Tag::PfLink {
                    link_code = None;
                    link_kind = None;
                }
                stack.push(tag);
            }
            Event::Empty(element) => {
                let tag = classify(element.local_name().as_ref());
                if tag == Tag::PfLink {
                    portfolios.push((String::new(), String::new()));
                }
            }
            Event::Text(text) => {
                let value = text
                    .unescape()
                    .map_err(|error| format!("SPAN XML text could not be decoded: {error}"))?;
                let parent = stack.last().copied();
                let grandparent = stack.get(stack.len().wrapping_sub(2)).copied();
                match (parent, grandparent) {
                    (Some(Tag::Cc), Some(Tag::CcDef)) => cc = Some(value.to_string()),
                    (Some(Tag::Name), Some(Tag::CcDef)) => name = Some(value.to_string()),
                    (Some(Tag::PfCode), Some(Tag::PfLink)) => link_code = Some(value.to_string()),
                    (Some(Tag::PfType), Some(Tag::PfLink)) => link_kind = Some(value.to_string()),
                    _ => {}
                }
            }
            Event::End(element) => {
                let tag = classify(element.local_name().as_ref());
                stack.pop();
                if tag == Tag::PfLink {
                    portfolios.push((
                        link_code.take().unwrap_or_default(),
                        link_kind.take().unwrap_or_default(),
                    ));
                }
                if tag == Tag::CcDef {
                    let Some(code) = cc.take() else {
                        return Err("SPAN combined commodity has no code".to_owned());
                    };
                    families.push(CommodityFamily {
                        cc: code,
                        name: name.take().unwrap_or_default(),
                        portfolios: std::mem::take(&mut portfolios),
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(families)
}

pub(super) fn extract_contracts(
    bytes: &[u8],
    trading_date: NaiveDate,
    interest: &[SymbolSpec],
) -> Result<Book, String> {
    let aliases = portfolio_aliases(bytes, interest)?;
    let mut state = ExtractState {
        interest,
        aliases,
        retain_all: false,
        spread_faults: Vec::new(),
        exchange: None,
        indexes: BTreeSet::new(),
        price_method: None,
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
    read_span(&mut state, bytes)?;
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
        exchange: state.exchange.unwrap_or_else(|| "NSE".to_owned()),
        contracts: state.contracts,
        commodities: state.commodities,
        underlyings: state.underlyings,
        spreads: state.spreads,
        indexes: state.indexes,
    })
}

pub(crate) fn parse_span_index(bytes: &[u8]) -> Result<SpanFileIndex, String> {
    let families = combined_commodities(bytes)?;
    let interest = Vec::new();
    let mut state = ExtractState {
        interest: &interest,
        aliases: BTreeMap::new(),
        retain_all: true,
        spread_faults: Vec::new(),
        exchange: None,
        indexes: BTreeSet::new(),
        price_method: None,
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
    read_span(&mut state, bytes)?;
    let business_date = state
        .business_date
        .ok_or_else(|| "SPAN business date is missing".to_owned())?;
    Ok(SpanFileIndex {
        business_date,
        exchange: state.exchange.unwrap_or_else(|| "NSE".to_owned()),
        families,
        contracts: state.contracts,
        commodities: state.commodities,
        underlyings: state.underlyings,
        spreads: state.spreads,
        indexes: state.indexes,
        spread_faults: state.spread_faults,
    })
}

fn read_span(state: &mut ExtractState<'_>, bytes: &[u8]) -> Result<(), String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("SPAN XML could not be read: {error}"))?;
        match event {
            Event::Start(element) => {
                let tag = classify(element.local_name().as_ref());
                open_extract(state, &stack, tag)?;
                stack.push(tag);
            }
            Event::Empty(element) => {
                let tag = classify(element.local_name().as_ref());
                open_extract(state, &stack, tag)?;
                close_extract(state, tag, stack.last().copied())?;
            }
            Event::Text(text) => {
                let value = text
                    .unescape()
                    .map_err(|error| format!("SPAN XML text could not be decoded: {error}"))?;
                if !value.is_empty() {
                    take_extract(state, &stack, &value)?;
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
                close_extract(state, tag, stack.last().copied())?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err("SPAN XML ended before all elements were closed".to_owned());
    }
    Ok(())
}

fn open_extract(state: &mut ExtractState<'_>, stack: &[Tag], tag: Tag) -> Result<(), String> {
    match tag {
        Tag::FutPf | Tag::OopPf | Tag::PhyPf => {
            state.product = None;
            state.price_method = None;
        }
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
        Tag::FutPf | Tag::PhyPf | Tag::OopPf => {
            if state.price_method.as_deref() == Some("IDX")
                && let Some(product) = state.product.clone()
            {
                state.indexes.insert(book_product(state, &product));
            }
            state.price_method = None;
        }
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

fn book_product(state: &ExtractState<'_>, file_code: &str) -> String {
    state
        .aliases
        .get(file_code)
        .cloned()
        .unwrap_or_else(|| file_code.to_owned())
}

fn finish_contract(state: &mut ExtractState<'_>, kind: ContractKind) -> Result<(), String> {
    let file_product = state
        .product
        .clone()
        .ok_or_else(|| format!("SPAN {} has no product code", kind.label()))?;
    let product = book_product(state, &file_product);
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
        && (state.retain_all || product_wanted(state.interest, &product))
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
    let keep =
        state.retain_all || requested(state.interest, kind, &product, state.expiry, state.strike);
    if keep {
        if state.values.is_empty() {
            return Err(format!(
                "SPAN {product} {} risk array was not retained",
                kind.label()
            ));
        }
        if listed_twice(&state.contracts, kind, &product, state.expiry, state.strike) {
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
    let name = book_product(state, &name);
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
    if state.retain_all {
        return true;
    }
    let Some(file_product) = state.product.as_deref() else {
        return true;
    };
    let product = book_product(state, file_product);
    match state.current {
        Some(Tag::Fut) => state
            .expiry
            .is_some_and(|expiry| wants_future(state.interest, &product, expiry)),
        Some(Tag::Opt) => match (state.expiry, state.right, state.strike) {
            (Some(expiry), Some('C'), Some(strike)) => {
                wants_option(state.interest, &product, true, strike, expiry)
            }
            (Some(expiry), Some('P'), Some(strike)) => {
                wants_option(state.interest, &product, false, strike, expiry)
            }
            _ => true,
        },
        Some(Tag::Phy) => wants_stock(state.interest, &product),
        _ => false,
    }
}

fn requested(
    interest: &[SymbolSpec],
    kind: ContractKind,
    product: &str,
    expiry: Option<NaiveDate>,
    strike: Option<Decimal>,
) -> bool {
    match kind {
        ContractKind::Future => {
            expiry.is_some_and(|expiry| wants_future(interest, product, expiry))
        }
        ContractKind::Call | ContractKind::Put => {
            let call = kind == ContractKind::Call;
            expiry.is_some_and(|expiry| {
                strike.is_some_and(|strike| wants_option(interest, product, call, strike, expiry))
            })
        }
        ContractKind::Stock => wants_stock(interest, product),
    }
}

fn listed_twice(
    contracts: &[SpanContract],
    kind: ContractKind,
    product: &str,
    expiry: Option<NaiveDate>,
    strike: Option<Decimal>,
) -> bool {
    contracts.iter().any(|contract| {
        contract.kind == kind
            && contract.product == product
            && contract.expiry == expiry
            && contract.strike == strike
    })
}

fn span_day(value: &str, what: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y%m%d")
        .map_err(|_| format!("SPAN {what} `{value}` is not YYYYMMDD"))
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
            let parsed = span_day(value, "business date")?;
            if state.business_date.replace(parsed).is_some() {
                return Err("SPAN business date is repeated".to_owned());
            }
        }
        Tag::PfCode if matches!(grandparent, Some(Tag::FutPf | Tag::OopPf | Tag::PhyPf)) => {
            state.product = Some(value.to_owned());
        }
        Tag::Expiry if grandparent == Some(Tag::Leg) => {
            state.leg_expiry = Some(span_day(value, "spread expiry")?);
        }
        Tag::Exch if value == "NSE" || value == "BSE" => match &state.exchange {
            Some(existing) if existing != value => {
                return Err(format!(
                    "SPAN exchange `{value}` conflicts with `{existing}`"
                ));
            }
            Some(_) => {}
            None => state.exchange = Some(value.to_owned()),
        },
        Tag::PriceMeth if matches!(grandparent, Some(Tag::FutPf | Tag::PhyPf | Tag::OopPf)) => {
            state.price_method = Some(value.to_owned());
        }
        Tag::Expiry if matches!(grandparent, Some(Tag::Fut | Tag::Phy)) => {
            if grandparent == Some(Tag::Phy) && (value == "00000000" || value == "000000") {
                state.expiry = None;
                state.expiry_seen = true;
            } else {
                state.expiry = Some(span_day(value, "expiry")?);
                state.expiry_seen = true;
            }
        }
        Tag::Expiry if grandparent == Some(Tag::Series) => {
            state.series_expiry = Some(span_day(value, "option series expiry")?);
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
    let products: Vec<String> = legs.iter().map(|(name, _, _)| name.clone()).collect();
    let wanted = state.retain_all
        || products
            .iter()
            .any(|name| product_wanted(state.interest, book_product(state, name).as_str()));
    if !wanted {
        state.spread_number = None;
        state.spread_method = None;
        state.spread_rate = None;
        return Ok(());
    }
    if legs.len() != 2 || legs[0].0 != legs[1].0 {
        return fail_spread(
            state,
            products,
            "SPAN intercommodity spread is not applied, and a calendar spread needs two legs of one product"
                .to_owned(),
        );
    }
    let product = book_product(state, &legs[0].0);
    let Some(number) = state.spread_number.take() else {
        return fail_spread(
            state,
            products,
            format!("SPAN calendar spread for {product} has no number"),
        );
    };
    let Some(method) = state.spread_method.take() else {
        return fail_spread(
            state,
            products,
            format!("SPAN calendar spread for {product} has no charge method"),
        );
    };
    let Some(charge) = state.spread_rate.take() else {
        return fail_spread(
            state,
            products,
            format!("SPAN calendar spread for {product} has no charge"),
        );
    };
    if state
        .spreads
        .get(&product)
        .is_some_and(|spreads| spreads.iter().any(|spread| spread.number == number))
    {
        return fail_spread(
            state,
            products,
            format!("SPAN calendar spread {number} for {product} is repeated"),
        );
    }
    let spreads = state.spreads.entry(product.clone()).or_default();
    spreads.push(CalendarSpread {
        number,
        charge,
        method,
        legs: [(legs[0].1, legs[0].2), (legs[1].1, legs[1].2)],
    });
    spreads.sort_by_key(|spread| spread.number);
    Ok(())
}

fn fail_spread(
    state: &mut ExtractState<'_>,
    products: Vec<String>,
    detail: String,
) -> Result<(), String> {
    state.spread_number = None;
    state.spread_method = None;
    state.spread_rate = None;
    if state.retain_all {
        state.spread_faults.push(SpreadFault { products, detail });
        Ok(())
    } else {
        Err(detail)
    }
}

pub(super) fn decimal(value: &str, what: &str) -> Result<Decimal, String> {
    Decimal::from_str(value).map_err(|_| format!("SPAN {what} `{value}` is not a decimal number"))
}

fn product_wanted(interest: &[SymbolSpec], product: &str) -> bool {
    interest.iter().any(|spec| spec.product() == product)
}
