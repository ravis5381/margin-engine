use crate::error::DomainError;
use crate::money::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Unit attached to every signed quantity. Conversions require [`QuantityConversion`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum QuantityUnit {
    #[serde(rename = "lot")]
    Lot,
    #[serde(rename = "contract")]
    Contract,
    #[serde(rename = "underlying_unit")]
    UnderlyingUnit,
}

impl QuantityUnit {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Lot => "lot",
            Self::Contract => "contract",
            Self::UnderlyingUnit => "underlying_unit",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "lot" => Ok(Self::Lot),
            "contract" => Ok(Self::Contract),
            "underlying_unit" => Ok(Self::UnderlyingUnit),
            other => Err(DomainError::QuantityConversionRejected {
                detail: format!("unknown quantity unit `{other}`"),
            }),
        }
    }
}

impl std::fmt::Display for QuantityUnit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

/// Signed quantity in one explicit unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignedQuantity {
    amount: Decimal,
    unit: QuantityUnit,
}

impl SignedQuantity {
    pub fn new(amount: Decimal, unit: QuantityUnit) -> Self {
        Self { amount, unit }
    }

    pub fn parse(amount: &str, unit: QuantityUnit) -> Result<Self, DomainError> {
        let money = Money::parse(amount, Currency::Inr)?;
        Ok(Self {
            amount: money.amount(),
            unit,
        })
    }

    pub fn amount(self) -> Decimal {
        self.amount
    }

    pub fn unit(self) -> QuantityUnit {
        self.unit
    }

    pub fn is_zero(self) -> bool {
        self.amount.is_zero()
    }

    pub fn checked_add(self, other: Self) -> Result<Self, DomainError> {
        if self.unit != other.unit {
            return Err(DomainError::QuantityUnitMismatch {
                left: self.unit,
                right: other.unit,
            });
        }
        let amount = self
            .amount
            .checked_add(other.amount)
            .ok_or(DomainError::MonetaryOverflow)?;
        Ok(Self {
            amount,
            unit: self.unit,
        })
    }
}

/// Explicit conversion between quantity units.
///
/// `factor` is dimensionless and must come from a named contract-master or rule reference.
/// Lot size on a contract is not applied automatically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantityConversion {
    from: QuantityUnit,
    to: QuantityUnit,
    factor: Decimal,
    rule_reference: String,
}

impl QuantityConversion {
    pub fn new(
        from: QuantityUnit,
        to: QuantityUnit,
        factor: Decimal,
        rule_reference: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let rule_reference = rule_reference.into();
        if from == to {
            return Err(DomainError::QuantityConversionRejected {
                detail: "conversion source and target units are the same".to_owned(),
            });
        }
        if factor <= Decimal::ZERO {
            return Err(DomainError::QuantityConversionRejected {
                detail: "conversion factor must be positive".to_owned(),
            });
        }
        if rule_reference.trim().is_empty() {
            return Err(DomainError::QuantityConversionRejected {
                detail: "conversion requires a non-empty rule or contract-master reference"
                    .to_owned(),
            });
        }
        Ok(Self {
            from,
            to,
            factor,
            rule_reference,
        })
    }

    pub fn apply(&self, quantity: SignedQuantity) -> Result<SignedQuantity, DomainError> {
        if quantity.unit() != self.from {
            return Err(DomainError::QuantityUnitMismatch {
                left: quantity.unit(),
                right: self.from,
            });
        }
        let amount = quantity
            .amount()
            .checked_mul(self.factor)
            .ok_or(DomainError::MonetaryOverflow)?;
        Ok(SignedQuantity::new(amount, self.to))
    }

    pub fn rule_reference(&self) -> &str {
        &self.rule_reference
    }
}
