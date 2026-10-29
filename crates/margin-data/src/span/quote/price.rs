//! Scanning risk, calendar spreads, extreme loss margin, premium, and delivery margin.

use super::file::{Book, CalendarSpread, ContractKind, Requested, canonical_symbol};
use super::inputs::QuoteInputs;
use super::rates::{
    CashMarginRates, ContractElmRates, ExposureRates, INDEX_DEEP_OTM_PERCENT, INDEX_ELM_PERCENT,
    INDEX_EXPIRY_EXTRA_PERCENT, INDEX_LONG_PERCENT, published_or_contract,
};
use super::symbol::SymbolSpec;
use chrono::{Datelike, NaiveDate, Weekday};
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const NOTE_BSE: &str = "SPAN scanning risk from the archived ICCL XML risk arrays. Loss is positive for a long unit. Scanning risk is the greatest scenario loss, and it is zero when every scenario is a gain. SENSEX is the combined commodity named BSE 30 SENSEX. Extreme loss margin for a future uses the total percent in the archived ICCL extreme-loss file for that future month and the futures settlement. A short option uses the IO or SO total percent for that expiry and strike, and the SPAN underlying price. A long option has no extreme loss margin. Delivery margin is zero for an index. The total is SPAN margin plus that extreme loss margin. Reported amounts are rounded half away from zero to paise. This quote does not replicate ICCL.";
pub(super) const NOTE: &str = "SPAN scanning risk from the archived XML risk arrays. Loss is positive for a long unit. Scanning risk is the greatest scenario loss, and it is zero when every scenario is a gain. Calendar spreads add the flat charge from the SPAN file and, for futures, extreme loss margin on one third of the far-month quantity. An index spread that includes a contract expiring on the trade date is not offset. Reported amounts are rounded half away from zero to paise. Delivery margin on a stock derivative is 10, 25, 45, or 70 percent of the archived cash-market daily margin rate from four trading days before expiry through the day before expiry, and the full rate on the expiry day. The quantity is the net shares that expiry would deliver. It is zero outside that window and for an index. The total is SPAN margin plus exposure margin plus delivery margin. Margin benefit compares the separate position totals with the total before delivery margin. Cross-margin benefit is zero.";
pub(super) const NOTE_SHORT_OPTION: &str = "SPAN scanning risk from the archived XML risk arrays. Loss is positive for a long unit. Scanning risk is the greatest scenario loss, and it is zero when every scenario is a gain. The short-option minimum rate in the SPAN file is zero. A short option's extreme loss margin uses the contract file's percent when that file is loaded, and otherwise the published index or exposure-file percent. Notional is the SPAN underlying price. Delivery margin on a stock derivative is 10, 25, 45, or 70 percent of the archived cash-market daily margin rate from four trading days before expiry through the day before expiry, and the full rate on the expiry day. The quantity is the net shares that expiry would deliver. The total is SPAN margin plus exposure margin plus delivery margin, and the short option premium is included in that total. A book of only long options has a total equal to its delivery margin. Margin benefit compares the separate position totals with the total before delivery margin. Cross-margin benefit is zero.";

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

pub(super) fn quote_book(
    book: &Book,
    inputs: QuoteInputs<'_>,
    specs: &[SymbolSpec],
) -> Result<SpanMarginQuote, String> {
    let requested = super::file::resolve_positions(inputs.positions, specs, book)?;
    price_book(inputs, &requested, book)
}

fn price_book(
    inputs: QuoteInputs<'_>,
    requested: &[Requested<'_>],
    book: &Book,
) -> Result<SpanMarginQuote, String> {
    let QuoteInputs {
        trading_date,
        positions,
        exposure,
        contracts,
        cash,
    } = inputs;
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
        let index = if book.indexes.contains(product) {
            true
        } else if book.exchange == "BSE" {
            false
        } else {
            !exposure.contains_stock(product)
        };
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
                canonical: canonical_symbol(&book.exchange, position.contract),
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
        note: if book.exchange == "BSE" {
            NOTE_BSE
        } else if short_option {
            NOTE_SHORT_OPTION
        } else {
            NOTE
        },
    })
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

pub(super) struct ExtremeLossInput<'a> {
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

pub(super) struct ExtremeLoss {
    amount: Decimal,
    percent: Decimal,
    underlying: Option<Decimal>,
}

fn contract_option_percent(
    input: &ExtremeLossInput<'_>,
    strike: Decimal,
    underlying: Decimal,
    expiry: NaiveDate,
) -> Result<Decimal, String> {
    if let Some(percent) =
        input
            .contracts
            .iccl_option_percent(input.product, input.kind, expiry, strike)
    {
        return percent;
    }
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
pub(super) struct FormedSpreads {
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

pub(super) fn paise(amount: Decimal) -> Decimal {
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
    if let Some((oth_percent, otm_percent)) = rate.exposure.option_percents(rate.product) {
        let deep = deep_otm(rate.kind, rate.strike, rate.underlying, 30)?;
        return Ok(if deep { otm_percent } else { oth_percent });
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

pub(super) struct ShortOptionRate<'a> {
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
