use crate::error::DomainError;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// ISO currency code. Indian equity derivatives in this engine are INR only.
///
/// There is no conversion constructor. Adding another currency requires an explicit
/// conversion policy, which this build does not contain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Currency {
    #[serde(rename = "INR")]
    Inr,
}

impl Currency {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Inr => "INR",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "INR" => Ok(Self::Inr),
            other => Err(DomainError::InvalidAmount {
                detail: format!("unsupported currency `{other}`"),
            }),
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// Where a rounding rule is allowed to change a monetary amount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundingPolicy {
    scale: u32,
    mode: RoundingMode,
    rule_reference: String,
}

impl RoundingPolicy {
    pub fn new(
        scale: u32,
        mode: RoundingMode,
        rule_reference: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let rule_reference = rule_reference.into();
        if rule_reference.trim().is_empty() {
            return Err(DomainError::InvalidAmount {
                detail: "rounding requires a non-empty official rule reference".to_owned(),
            });
        }
        if scale > 28 {
            return Err(DomainError::InvalidAmount {
                detail: "rounding scale cannot exceed 28 decimal places".to_owned(),
            });
        }
        Ok(Self {
            scale,
            mode,
            rule_reference,
        })
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    pub fn mode(&self) -> RoundingMode {
        self.mode
    }

    pub fn rule_reference(&self) -> &str {
        &self.rule_reference
    }
}

/// Rounding modes the caller must choose explicitly. This crate does not pick one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundingMode {
    HalfAwayFromZero,
    HalfEven,
    TowardZero,
    AwayFromZero,
}

impl RoundingMode {
    fn strategy(self) -> rust_decimal::RoundingStrategy {
        match self {
            Self::HalfAwayFromZero => rust_decimal::RoundingStrategy::MidpointAwayFromZero,
            Self::HalfEven => rust_decimal::RoundingStrategy::MidpointNearestEven,
            Self::TowardZero => rust_decimal::RoundingStrategy::ToZero,
            Self::AwayFromZero => rust_decimal::RoundingStrategy::AwayFromZero,
        }
    }
}

/// Currency-tagged decimal amount.
///
/// Addition does not round. Call [`Money::round`] only at a boundary named by a
/// [`RoundingPolicy`]. Amounts serialize as decimal strings, never JSON numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Money {
    amount: Decimal,
    currency: Currency,
}

impl Money {
    pub fn parse(amount: &str, currency: Currency) -> Result<Self, DomainError> {
        Ok(Self {
            amount: parse_decimal_token(amount)?,
            currency,
        })
    }

    pub fn from_decimal(amount: Decimal, currency: Currency) -> Self {
        Self { amount, currency }
    }

    pub fn from_minor_units(
        minor: i64,
        scale: u32,
        currency: Currency,
    ) -> Result<Self, DomainError> {
        if scale > 28 {
            return Err(DomainError::InvalidAmount {
                detail: "minor-unit scale cannot exceed 28".to_owned(),
            });
        }
        Ok(Self {
            amount: Decimal::from_i128_with_scale(i128::from(minor), scale),
            currency,
        })
    }

    pub fn zero(currency: Currency) -> Self {
        Self {
            amount: Decimal::ZERO,
            currency,
        }
    }

    pub fn amount(self) -> Decimal {
        self.amount
    }

    pub fn currency(self) -> Currency {
        self.currency
    }

    pub fn is_zero(self) -> bool {
        self.amount.is_zero()
    }

    pub fn is_negative(self) -> bool {
        self.amount.is_sign_negative()
    }

    pub fn checked_add(self, other: Self) -> Result<Self, DomainError> {
        self.same_currency(other)?;
        let amount = self
            .amount
            .checked_add(other.amount)
            .ok_or(DomainError::MonetaryOverflow)?;
        Ok(Self {
            amount,
            currency: self.currency,
        })
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, DomainError> {
        self.same_currency(other)?;
        let amount = self
            .amount
            .checked_sub(other.amount)
            .ok_or(DomainError::MonetaryOverflow)?;
        Ok(Self {
            amount,
            currency: self.currency,
        })
    }

    /// Multiplies by a dimensionless exact factor. This is not a currency conversion.
    pub fn checked_mul(self, factor: Decimal) -> Result<Self, DomainError> {
        let amount = self
            .amount
            .checked_mul(factor)
            .ok_or(DomainError::MonetaryOverflow)?;
        Ok(Self {
            amount,
            currency: self.currency,
        })
    }

    pub fn checked_neg(self) -> Result<Self, DomainError> {
        self.checked_mul(Decimal::NEGATIVE_ONE)
    }

    pub fn round(self, policy: &RoundingPolicy) -> Result<Self, DomainError> {
        Ok(Self {
            amount: self
                .amount
                .round_dp_with_strategy(policy.scale, policy.mode.strategy()),
            currency: self.currency,
        })
    }

    fn same_currency(self, other: Self) -> Result<(), DomainError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(DomainError::CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            })
        }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.amount, self.currency)
    }
}

impl Serialize for Money {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Money", 2)?;
        state.serialize_field("amount", &self.amount.to_string())?;
        state.serialize_field("currency", &self.currency)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawMoney {
            amount: String,
            currency: Currency,
        }
        let raw = RawMoney::deserialize(deserializer)?;
        Money::parse(&raw.amount, raw.currency).map_err(serde::de::Error::custom)
    }
}

fn parse_decimal_token(raw: &str) -> Result<Decimal, DomainError> {
    if raw.is_empty()
        || raw.chars().any(|character| {
            character.is_whitespace() || matches!(character, 'e' | 'E' | 'f' | 'F')
        })
    {
        return Err(DomainError::InvalidAmount {
            detail: format!("decimal token `{raw}` is empty or uses exponent notation"),
        });
    }
    Decimal::from_str(raw).map_err(|_| DomainError::InvalidAmount {
        detail: format!("cannot parse decimal `{raw}`"),
    })
}

#[cfg(test)]
mod tests {
    use super::{Currency, Money, RoundingMode, RoundingPolicy};
    use crate::error::DomainError;
    use rust_decimal::Decimal;

    fn inr(amount: &str) -> Money {
        Money::parse(amount, Currency::Inr).expect("amount")
    }

    #[test]
    fn addition_keeps_exact_decimal_tenths() {
        let sum = inr("0.1").checked_add(inr("0.2")).expect("sum");
        assert_eq!(sum.amount().to_string(), "0.3");
    }

    #[test]
    fn addition_does_not_round_to_paise() {
        let sum = inr("0.014").checked_add(inr("0.014")).expect("sum");
        assert_eq!(sum.amount().to_string(), "0.028");
    }

    #[test]
    fn json_amount_must_be_a_string() {
        let money = inr("1.50");
        assert_eq!(
            serde_json::to_string(&money).expect("serialize"),
            r#"{"amount":"1.50","currency":"INR"}"#
        );
        assert!(serde_json::from_str::<Money>(r#"{"amount":1.50,"currency":"INR"}"#).is_err());
    }

    #[test]
    fn rounding_requires_a_rule_reference_and_is_explicit() {
        assert!(RoundingPolicy::new(2, RoundingMode::HalfAwayFromZero, "  ").is_err());
        let policy = RoundingPolicy::new(
            2,
            RoundingMode::HalfAwayFromZero,
            "test-only rounding boundary",
        )
        .expect("policy");
        let rounded = inr("1.225").round(&policy).expect("round");
        assert_eq!(rounded.amount().to_string(), "1.23");
        let half_even =
            RoundingPolicy::new(0, RoundingMode::HalfEven, "test-only half even").expect("policy");
        assert_eq!(
            inr("2.5").round(&half_even).expect("round").amount(),
            Decimal::from(2)
        );
    }

    #[test]
    fn overflow_and_missing_currency_conversion_are_errors() {
        let max = Money::from_decimal(Decimal::MAX, Currency::Inr);
        let one = Money::from_decimal(Decimal::ONE, Currency::Inr);
        assert!(matches!(
            max.checked_add(one),
            Err(DomainError::MonetaryOverflow)
        ));
        assert!(Currency::parse("USD").is_err());
    }

    #[test]
    fn minor_units_do_not_use_binary_floats() {
        let amount = Money::from_minor_units(1050, 2, Currency::Inr).expect("paise");
        assert_eq!(amount.amount().to_string(), "10.50");
    }
}
