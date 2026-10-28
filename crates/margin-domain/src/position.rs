use crate::clearing::{ClearingCorporation, Exchange};
use crate::contract::Contract;
use crate::error::DomainError;
use crate::ids::{AggregationScopeId, ContractId, PortfolioId};
use crate::money::{Currency, Money};
use crate::quantity::SignedQuantity;
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::BTreeMap;

/// Kind of account grouping. A permitted group must name the rule that allows it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AggregationScopeKind {
    ClientAccount,
    Portfolio,
    PermittedGroup { rule_reference: String },
}

impl AggregationScopeKind {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ClientAccount => "client_account",
            Self::Portfolio => "portfolio",
            Self::PermittedGroup { .. } => "permitted_group",
        }
    }

    pub fn rule_reference(&self) -> Option<&str> {
        match self {
            Self::PermittedGroup { rule_reference } => Some(rule_reference),
            Self::ClientAccount | Self::Portfolio => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AggregationScope {
    id: AggregationScopeId,
    kind: AggregationScopeKind,
}

impl AggregationScope {
    pub fn new(id: AggregationScopeId, kind: AggregationScopeKind) -> Result<Self, DomainError> {
        if let AggregationScopeKind::PermittedGroup { rule_reference } = &kind
            && rule_reference.trim().is_empty()
        {
            return Err(DomainError::InvalidIdentifier {
                detail: "permitted aggregation group requires a rule reference".to_owned(),
            });
        }
        Ok(Self { id, kind })
    }

    pub fn id(&self) -> &AggregationScopeId {
        &self.id
    }

    pub fn kind(&self) -> &AggregationScopeKind {
        &self.kind
    }
}

/// Price used for option value. The source reference must name an official valuation source.
///
/// Last traded price is not a variant. Callers cannot record an unofficial print as if it
/// were the clearing valuation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValuationPrice {
    price: Money,
    source_reference: String,
    as_of: DateTime<Utc>,
}

impl ValuationPrice {
    pub fn try_new(
        price: Money,
        source_reference: impl Into<String>,
        as_of: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        let source_reference = source_reference.into();
        if source_reference.trim().is_empty() {
            return Err(DomainError::InvalidAmount {
                detail: "valuation price requires an official source reference".to_owned(),
            });
        }
        Ok(Self {
            price,
            source_reference,
            as_of,
        })
    }

    pub fn price(&self) -> Money {
        self.price
    }

    pub fn source_reference(&self) -> &str {
        &self.source_reference
    }

    pub fn as_of(&self) -> DateTime<Utc> {
        self.as_of
    }
}

pub struct PositionDraft {
    pub portfolio_id: PortfolioId,
    pub aggregation_scope: AggregationScope,
    pub contract_id: ContractId,
    pub exchange: Exchange,
    pub quantity: SignedQuantity,
    pub average_trade_price: Option<Money>,
    pub valuation: Option<ValuationPrice>,
    pub origin: String,
    pub timestamp: DateTime<Utc>,
    pub clearing_corporation: ClearingCorporation,
    pub trading_date: NaiveDate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    portfolio_id: PortfolioId,
    aggregation_scope: AggregationScope,
    contract_id: ContractId,
    exchange: Exchange,
    quantity: SignedQuantity,
    average_trade_price: Option<Money>,
    valuation: Option<ValuationPrice>,
    origin: String,
    timestamp: DateTime<Utc>,
    clearing_corporation: ClearingCorporation,
    trading_date: NaiveDate,
}

impl Position {
    pub fn try_new(draft: PositionDraft) -> Result<Self, DomainError> {
        if draft.origin.trim().is_empty() || draft.origin.len() > 64 {
            return Err(DomainError::InvalidIdentifier {
                detail: "position origin must be 1 to 64 non-blank characters".to_owned(),
            });
        }
        if let Some(price) = draft.average_trade_price
            && price.currency() != Currency::Inr
        {
            return Err(DomainError::InvalidAmount {
                detail: "average trade price must be INR".to_owned(),
            });
        }
        Ok(Self {
            portfolio_id: draft.portfolio_id,
            aggregation_scope: draft.aggregation_scope,
            contract_id: draft.contract_id,
            exchange: draft.exchange,
            quantity: draft.quantity,
            average_trade_price: draft.average_trade_price,
            valuation: draft.valuation,
            origin: draft.origin,
            timestamp: draft.timestamp,
            clearing_corporation: draft.clearing_corporation,
            trading_date: draft.trading_date,
        })
    }

    pub fn portfolio_id(&self) -> &PortfolioId {
        &self.portfolio_id
    }

    pub fn aggregation_scope(&self) -> &AggregationScope {
        &self.aggregation_scope
    }

    pub fn contract_id(&self) -> &ContractId {
        &self.contract_id
    }

    pub fn exchange(&self) -> Exchange {
        self.exchange
    }

    pub fn quantity(&self) -> SignedQuantity {
        self.quantity
    }

    pub fn average_trade_price(&self) -> Option<Money> {
        self.average_trade_price
    }

    pub fn valuation(&self) -> Option<&ValuationPrice> {
        self.valuation.as_ref()
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    pub fn clearing_corporation(&self) -> ClearingCorporation {
        self.clearing_corporation
    }

    pub fn trading_date(&self) -> NaiveDate {
        self.trading_date
    }

    pub fn ensure_matches_contract(&self, contract: &Contract) -> Result<(), DomainError> {
        if self.contract_id != *contract.id() {
            return Err(DomainError::InvalidContract {
                detail: format!(
                    "position contract {} does not match contract {}",
                    self.contract_id,
                    contract.id()
                ),
            });
        }
        if self.exchange != contract.exchange()
            || self.clearing_corporation != contract.clearing_corporation()
        {
            return Err(DomainError::ConflictingPositionAttributes {
                contract_id: self.contract_id.to_string(),
                detail: "position exchange or clearing corporation does not match the contract"
                    .to_owned(),
            });
        }
        Ok(())
    }
}

/// Scope shared by every non-zero position in a normalized book.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalculationScope {
    pub portfolio_id: PortfolioId,
    pub aggregation_scope: AggregationScope,
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub trading_date: NaiveDate,
}

/// Positions combined by contract. Zero quantities are omitted and do not affect the book.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedBook {
    positions: Vec<Position>,
    ignored_zero_quantity: usize,
}

impl NormalizedBook {
    pub fn from_positions(positions: &[Position]) -> Result<Self, DomainError> {
        let ignored_zero_quantity = positions
            .iter()
            .filter(|position| position.quantity.is_zero())
            .count();
        let mut grouped: BTreeMap<IdentityKey, Position> = BTreeMap::new();
        for position in positions
            .iter()
            .filter(|position| !position.quantity.is_zero())
        {
            let key = IdentityKey::from_position(position);
            if let Some(existing) = grouped.get_mut(&key) {
                if !economics_match(existing, position) {
                    return Err(DomainError::ConflictingPositionAttributes {
                        contract_id: position.contract_id.to_string(),
                        detail: "quantity cannot be combined because price, unit, valuation, or origin differs"
                            .to_owned(),
                    });
                }
                existing.quantity = existing.quantity.checked_add(position.quantity)?;
                if position.timestamp > existing.timestamp {
                    existing.timestamp = position.timestamp;
                }
            } else {
                grouped.insert(key, position.clone());
            }
        }
        let positions = grouped
            .into_values()
            .filter(|position| !position.quantity.is_zero())
            .collect();
        Ok(Self {
            positions,
            ignored_zero_quantity,
        })
    }

    pub fn positions(&self) -> &[Position] {
        &self.positions
    }

    pub fn ignored_zero_quantity(&self) -> usize {
        self.ignored_zero_quantity
    }

    pub fn single_scope(&self) -> Result<CalculationScope, DomainError> {
        let Some(first) = self.positions.first() else {
            return Err(DomainError::MissingRequiredData {
                detail: "normalized book has no non-zero positions".to_owned(),
            });
        };
        for position in &self.positions {
            if position.portfolio_id != first.portfolio_id
                || position.aggregation_scope != first.aggregation_scope
                || position.exchange != first.exchange
                || position.clearing_corporation != first.clearing_corporation
                || position.trading_date != first.trading_date
            {
                return Err(DomainError::ConflictingPositionAttributes {
                    contract_id: position.contract_id.to_string(),
                    detail: "portfolio, aggregation scope, exchange, clearing corporation, or trade date differs"
                        .to_owned(),
                });
            }
        }
        Ok(CalculationScope {
            portfolio_id: first.portfolio_id.clone(),
            aggregation_scope: first.aggregation_scope.clone(),
            exchange: first.exchange,
            clearing_corporation: first.clearing_corporation,
            trading_date: first.trading_date,
        })
    }
}

fn economics_match(left: &Position, right: &Position) -> bool {
    left.quantity.unit() == right.quantity.unit()
        && left.origin == right.origin
        && prices_match(left.average_trade_price, right.average_trade_price)
        && left.valuation == right.valuation
}

fn prices_match(left: Option<Money>, right: Option<Money>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct IdentityKey {
    portfolio: String,
    scope_kind: u8,
    scope_rule: String,
    scope_id: String,
    contract_id: String,
    exchange: Exchange,
    clearing: ClearingCorporation,
    trading_date: NaiveDate,
}

impl IdentityKey {
    fn from_position(position: &Position) -> Self {
        let (scope_kind, scope_rule) = match position.aggregation_scope.kind() {
            AggregationScopeKind::ClientAccount => (0, String::new()),
            AggregationScopeKind::Portfolio => (1, String::new()),
            AggregationScopeKind::PermittedGroup { rule_reference } => (2, rule_reference.clone()),
        };
        Self {
            portfolio: position.portfolio_id.as_str().to_owned(),
            scope_kind,
            scope_rule,
            scope_id: position.aggregation_scope.id().as_str().to_owned(),
            contract_id: position.contract_id.as_str().to_owned(),
            exchange: position.exchange,
            clearing: position.clearing_corporation,
            trading_date: position.trading_date,
        }
    }
}
