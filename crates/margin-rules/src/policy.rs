use chrono::NaiveDate;
use margin_domain::{
    ClearingCorporation, DomainError, Exchange, InstrumentType, Money, NormalizedBook,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RulesError {
    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("margin policy component `{component}` is not implemented: {detail}")]
    SpecificationRequired {
        component: &'static str,
        detail: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrumentScope {
    Instrument(InstrumentType),
    AllListedEquityDerivatives,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarginRule {
    id: margin_domain::RuleVersionId,
    clearing_corporation: ClearingCorporation,
    exchange: Option<Exchange>,
    product: InstrumentScope,
    effective_from: NaiveDate,
    effective_to: Option<NaiveDate>,
    source_reference: String,
}

impl MarginRule {
    pub fn try_new(
        id: margin_domain::RuleVersionId,
        clearing_corporation: ClearingCorporation,
        exchange: Option<Exchange>,
        product: InstrumentScope,
        effective_from: NaiveDate,
        effective_to: Option<NaiveDate>,
        source_reference: impl Into<String>,
    ) -> Result<Self, RulesError> {
        let source_reference = source_reference.into();
        if source_reference.trim().is_empty() {
            return Err(RulesError::Domain(DomainError::MissingRequiredData {
                detail: "margin rule requires a source reference".to_owned(),
            }));
        }
        if let Some(effective_to) = effective_to
            && effective_to < effective_from
        {
            return Err(RulesError::Domain(DomainError::MissingRequiredData {
                detail: "margin rule effective_to is before effective_from".to_owned(),
            }));
        }
        Ok(Self {
            id,
            clearing_corporation,
            exchange,
            product,
            effective_from,
            effective_to,
            source_reference,
        })
    }

    pub fn id(&self) -> &margin_domain::RuleVersionId {
        &self.id
    }

    pub fn source_reference(&self) -> &str {
        &self.source_reference
    }

    pub fn effective_from(&self) -> NaiveDate {
        self.effective_from
    }

    fn covers(&self, on: NaiveDate) -> bool {
        on >= self.effective_from && self.effective_to.is_none_or(|end| on <= end)
    }
}

#[derive(Clone, Debug, Default)]
pub struct MarginRuleRegistry {
    rules: Vec<MarginRule>,
}

impl MarginRuleRegistry {
    pub fn empty() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn register(&mut self, rule: MarginRule) -> Result<(), RulesError> {
        if self.rules.iter().any(|existing| existing.id == rule.id) {
            return Err(RulesError::Domain(DomainError::AmbiguousDataset {
                detail: format!("margin rule {} is already registered", rule.id),
            }));
        }
        self.rules.push(rule);
        Ok(())
    }
}

pub struct RuleQuery {
    pub clearing_corporation: ClearingCorporation,
    pub exchange: Exchange,
    pub product: InstrumentType,
    pub on: NaiveDate,
}

pub fn select_rule<'a>(
    registry: &'a MarginRuleRegistry,
    query: &RuleQuery,
) -> Result<&'a MarginRule, RulesError> {
    let matches: Vec<&MarginRule> = registry
        .rules
        .iter()
        .filter(|rule| {
            rule.clearing_corporation == query.clearing_corporation
                && rule
                    .exchange
                    .is_none_or(|exchange| exchange == query.exchange)
                && matches_product(rule.product, query.product)
                && rule.covers(query.on)
        })
        .collect();
    match matches.as_slice() {
        [rule] => Ok(*rule),
        [] => Err(RulesError::Domain(DomainError::MissingRequiredData {
            detail: format!(
                "no margin rule is registered for {} {} {} on {}",
                query.clearing_corporation, query.exchange, query.product, query.on
            ),
        })),
        _ => Err(RulesError::Domain(DomainError::AmbiguousDataset {
            detail: "more than one margin rule matches the query".to_owned(),
        })),
    }
}

fn matches_product(scope: InstrumentScope, product: InstrumentType) -> bool {
    match scope {
        InstrumentScope::AllListedEquityDerivatives => true,
        InstrumentScope::Instrument(instrument) => instrument == product,
    }
}

pub struct PolicyInputs<'a> {
    pub book: &'a NormalizedBook,
}

pub fn apply_margin_policy(
    rule: &MarginRule,
    inputs: &PolicyInputs<'_>,
) -> Result<Money, RulesError> {
    let _ = (rule, inputs);
    Err(RulesError::SpecificationRequired {
        component: "margin_policy",
        detail: "Selecting a rule record does not calculate a charge. No component formula is implemented.",
    })
}

pub fn elm(inputs: &PolicyInputs<'_>) -> Result<Money, RulesError> {
    reject("elm", inputs)
}

pub fn delivery_margin(inputs: &PolicyInputs<'_>) -> Result<Money, RulesError> {
    reject("delivery_margin", inputs)
}

pub fn premium_obligation(inputs: &PolicyInputs<'_>) -> Result<Money, RulesError> {
    reject("premium_obligation", inputs)
}

pub fn crystallised_obligations(inputs: &PolicyInputs<'_>) -> Result<Money, RulesError> {
    reject("crystallised_obligations", inputs)
}

pub fn additional_margin(inputs: &PolicyInputs<'_>) -> Result<Money, RulesError> {
    reject("additional_margin", inputs)
}

pub fn cross_margin_benefit(
    permitted_flag: bool,
    inputs: &PolicyInputs<'_>,
) -> Result<Money, RulesError> {
    let _ = inputs;
    Err(RulesError::SpecificationRequired {
        component: "cross_margin_benefit",
        detail: if permitted_flag {
            "Cross-exchange margin benefits are not implemented. A permitted flag does not calculate a benefit."
        } else {
            "Cross-exchange margin benefits are not implemented. The position is not flagged as eligible."
        },
    })
}

fn reject(component: &'static str, inputs: &PolicyInputs<'_>) -> Result<Money, RulesError> {
    let _ = inputs;
    Err(RulesError::SpecificationRequired {
        component,
        detail: "No official charge schedule is implemented for this component.",
    })
}
