use crate::error::DomainError;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Trading venue. This does not select a clearing corporation by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Exchange {
    #[serde(rename = "NSE")]
    Nse,
    #[serde(rename = "BSE")]
    Bse,
}

impl Exchange {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Nse => "NSE",
            Self::Bse => "BSE",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "NSE" => Ok(Self::Nse),
            "BSE" => Ok(Self::Bse),
            other => Err(DomainError::InvalidIdentifier {
                detail: format!("unknown exchange `{other}`"),
            }),
        }
    }
}

impl std::fmt::Display for Exchange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

/// Clearing corporation.
///
/// `Nscc` is the internal name requested for NSE Clearing Limited. External files and APIs
/// use `NSCCL`, which remains the code used by SPAN and interoperability material.
/// `Iccl` is Indian Clearing Corporation Ltd.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ClearingCorporation {
    #[serde(rename = "NSCCL")]
    Nscc,
    #[serde(rename = "ICCL")]
    Iccl,
}

impl ClearingCorporation {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Nscc => "NSCCL",
            Self::Iccl => "ICCL",
        }
    }

    pub const fn legal_name(self) -> &'static str {
        match self {
            Self::Nscc => "NSE Clearing Limited",
            Self::Iccl => "Indian Clearing Corporation Ltd.",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "NSCCL" => Ok(Self::Nscc),
            "ICCL" => Ok(Self::Iccl),
            other => Err(DomainError::InvalidIdentifier {
                detail: format!("unknown clearing corporation `{other}`"),
            }),
        }
    }
}

impl std::fmt::Display for ClearingCorporation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

/// One configured exchange-to-clearing relationship.
///
/// `cross_exchange_offset_permitted` is a configuration flag only. It does not calculate
/// or grant a margin offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearingArrangement {
    exchange: Exchange,
    clearing_corporation: ClearingCorporation,
    effective_from: NaiveDate,
    effective_to: Option<NaiveDate>,
    cross_exchange_offset_permitted: bool,
    rule_reference: String,
}

impl ClearingArrangement {
    pub fn try_new(
        exchange: Exchange,
        clearing_corporation: ClearingCorporation,
        effective_from: NaiveDate,
        effective_to: Option<NaiveDate>,
        cross_exchange_offset_permitted: bool,
        rule_reference: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let rule_reference = rule_reference.into();
        if rule_reference.trim().is_empty() {
            return Err(DomainError::InvalidIdentifier {
                detail: "clearing arrangement requires a non-empty rule reference".to_owned(),
            });
        }
        if let Some(effective_to) = effective_to
            && effective_to < effective_from
        {
            return Err(DomainError::InvalidIdentifier {
                detail: "clearing arrangement effective_to is before effective_from".to_owned(),
            });
        }
        Ok(Self {
            exchange,
            clearing_corporation,
            effective_from,
            effective_to,
            cross_exchange_offset_permitted,
            rule_reference,
        })
    }

    pub fn exchange(&self) -> Exchange {
        self.exchange
    }

    pub fn clearing_corporation(&self) -> ClearingCorporation {
        self.clearing_corporation
    }

    pub fn effective_from(&self) -> NaiveDate {
        self.effective_from
    }

    pub fn effective_to(&self) -> Option<NaiveDate> {
        self.effective_to
    }

    pub fn cross_exchange_offset_permitted(&self) -> bool {
        self.cross_exchange_offset_permitted
    }

    pub fn rule_reference(&self) -> &str {
        &self.rule_reference
    }

    fn covers(&self, on: NaiveDate) -> bool {
        on >= self.effective_from && self.effective_to.is_none_or(|end| on <= end)
    }
}

/// Operator-supplied clearing relationships. An empty configuration authorizes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ClearingConfiguration {
    arrangements: Vec<ClearingArrangement>,
}

impl ClearingConfiguration {
    pub fn empty() -> Self {
        Self {
            arrangements: Vec::new(),
        }
    }

    pub fn try_from_arrangements(
        arrangements: Vec<ClearingArrangement>,
    ) -> Result<Self, DomainError> {
        for (index, arrangement) in arrangements.iter().enumerate() {
            for other in arrangements.iter().skip(index + 1) {
                if arrangement.exchange == other.exchange
                    && arrangement.clearing_corporation == other.clearing_corporation
                    && ranges_overlap(arrangement, other)
                {
                    return Err(DomainError::AmbiguousDataset {
                        detail: format!(
                            "overlapping clearing arrangements for {} / {}",
                            arrangement.exchange, arrangement.clearing_corporation
                        ),
                    });
                }
            }
        }
        Ok(Self { arrangements })
    }

    pub fn authorize(
        &self,
        exchange: Exchange,
        clearing_corporation: ClearingCorporation,
        on: NaiveDate,
    ) -> Result<&ClearingArrangement, DomainError> {
        let matches: Vec<_> = self
            .arrangements
            .iter()
            .filter(|arrangement| {
                arrangement.exchange == exchange
                    && arrangement.clearing_corporation == clearing_corporation
                    && arrangement.covers(on)
            })
            .collect();
        match matches.as_slice() {
            [arrangement] => Ok(*arrangement),
            [] => Err(DomainError::ClearingArrangementRejected {
                exchange,
                clearing_corporation,
                trade_date: on,
            }),
            _ => Err(DomainError::AmbiguousDataset {
                detail: format!(
                    "multiple clearing arrangements match {exchange} / {clearing_corporation} on {on}"
                ),
            }),
        }
    }

    pub fn arrangements(&self) -> &[ClearingArrangement] {
        &self.arrangements
    }
}

fn ranges_overlap(left: &ClearingArrangement, right: &ClearingArrangement) -> bool {
    let far = NaiveDate::from_ymd_opt(9999, 12, 31).expect("valid sentinel date");
    let left_end = left.effective_to.unwrap_or(far);
    let right_end = right.effective_to.unwrap_or(far);
    left.effective_from <= right_end && right.effective_from <= left_end
}

/// Evidence the caller would need before a cross-exchange offset could be considered.
///
/// [`assess_cross_exchange_offset`] rejects every variant in this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OffsetEligibility {
    NotProvided,
    Rejected { reason: String },
    Configured { rule_reference: String },
}

pub fn assess_cross_exchange_offset(eligibility: &OffsetEligibility) -> Result<(), DomainError> {
    let detail = match eligibility {
        OffsetEligibility::NotProvided => {
            "cross-exchange offset eligibility was not provided".to_owned()
        }
        OffsetEligibility::Rejected { reason } => {
            format!("cross-exchange offset rejected: {reason}")
        }
        OffsetEligibility::Configured { rule_reference } => format!(
            "cross-exchange offset is not applied; reference `{rule_reference}` does not authorize netting in this build"
        ),
    };
    Err(DomainError::OffsetRejected { detail })
}
