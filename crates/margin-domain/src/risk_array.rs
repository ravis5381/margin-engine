use crate::error::DomainError;
use crate::ids::{ContractId, DatasetId, ScenarioId};
use crate::money::Money;
use crate::quantity::QuantityUnit;
use rust_decimal::Decimal;
use std::collections::BTreeSet;

/// Meaning of a source scenario number before canonicalization.
///
/// The parser must set this from the format specification. There is no default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueConvention {
    /// Positive amount means profit for a long unit.
    ProfitAndLoss,
    /// Positive amount means loss for a long unit. Normalization negates it once.
    Loss,
}

/// One source scenario before it becomes canonical P&L.
pub struct ScenarioValueDraft {
    pub scenario_id: ScenarioId,
    pub official_index: u16,
    pub amount: Money,
    pub convention: ValueConvention,
    pub quantity_basis: QuantityUnit,
    pub scale: Decimal,
    pub source_raw: String,
    pub source_locator: Option<String>,
}

pub struct RiskArrayDraft {
    pub contract_id: ContractId,
    pub dataset_id: DatasetId,
    pub quantity_basis: QuantityUnit,
    pub scenarios: Vec<ScenarioValueDraft>,
}

/// Canonical P&L for one scenario. `pnl_per_unit` already includes sign conversion and scale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedScenario {
    scenario_id: ScenarioId,
    official_index: u16,
    pnl_per_unit: Money,
    quantity_basis: QuantityUnit,
    source_raw: String,
    source_locator: Option<String>,
    normalized_from: ValueConvention,
    applied_scale: Decimal,
}

impl NormalizedScenario {
    pub fn scenario_id(&self) -> &ScenarioId {
        &self.scenario_id
    }

    pub fn official_index(&self) -> u16 {
        self.official_index
    }

    pub fn pnl_per_unit(&self) -> Money {
        self.pnl_per_unit
    }

    pub fn quantity_basis(&self) -> QuantityUnit {
        self.quantity_basis
    }

    pub fn source_raw(&self) -> &str {
        &self.source_raw
    }

    pub fn source_locator(&self) -> Option<&str> {
        self.source_locator.as_deref()
    }

    pub fn normalized_from(&self) -> ValueConvention {
        self.normalized_from
    }

    pub fn applied_scale(&self) -> Decimal {
        self.applied_scale
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedRiskArray {
    contract_id: ContractId,
    dataset_id: DatasetId,
    quantity_basis: QuantityUnit,
    scenarios: Vec<NormalizedScenario>,
}

impl NormalizedRiskArray {
    pub fn try_new(draft: RiskArrayDraft) -> Result<Self, DomainError> {
        if draft.scenarios.is_empty() {
            return Err(DomainError::RiskArrayRejected {
                detail: format!("contract {} has no scenario values", draft.contract_id),
            });
        }
        let mut seen = BTreeSet::new();
        let mut previous_index: Option<u16> = None;
        let mut currency = None;
        let mut scenarios = Vec::with_capacity(draft.scenarios.len());
        for scenario in draft.scenarios {
            if scenario.quantity_basis != draft.quantity_basis {
                return Err(DomainError::QuantityUnitMismatch {
                    left: scenario.quantity_basis,
                    right: draft.quantity_basis,
                });
            }
            if !seen.insert(scenario.scenario_id.clone()) {
                return Err(DomainError::RiskArrayRejected {
                    detail: format!("duplicate scenario id {}", scenario.scenario_id),
                });
            }
            if let Some(previous) = previous_index
                && scenario.official_index <= previous
            {
                return Err(DomainError::RiskArrayRejected {
                    detail: "scenario official indexes must be strictly increasing".to_owned(),
                });
            }
            previous_index = Some(scenario.official_index);
            if scenario.scale <= Decimal::ZERO {
                return Err(DomainError::RiskArrayRejected {
                    detail: format!("scenario {} scale must be positive", scenario.scenario_id),
                });
            }
            if scenario.source_raw.trim().is_empty() {
                return Err(DomainError::RiskArrayRejected {
                    detail: format!(
                        "scenario {} is missing its original source value",
                        scenario.scenario_id
                    ),
                });
            }
            match currency {
                None => currency = Some(scenario.amount.currency()),
                Some(expected) if expected != scenario.amount.currency() => {
                    return Err(DomainError::CurrencyMismatch {
                        left: expected,
                        right: scenario.amount.currency(),
                    });
                }
                Some(_) => {}
            }
            let signed = match scenario.convention {
                ValueConvention::ProfitAndLoss => scenario.amount,
                ValueConvention::Loss => scenario.amount.checked_neg()?,
            };
            let pnl_per_unit = signed.checked_mul(scenario.scale)?;
            scenarios.push(NormalizedScenario {
                scenario_id: scenario.scenario_id,
                official_index: scenario.official_index,
                pnl_per_unit,
                quantity_basis: scenario.quantity_basis,
                source_raw: scenario.source_raw,
                source_locator: scenario.source_locator,
                normalized_from: scenario.convention,
                applied_scale: scenario.scale,
            });
        }
        Ok(Self {
            contract_id: draft.contract_id,
            dataset_id: draft.dataset_id,
            quantity_basis: draft.quantity_basis,
            scenarios,
        })
    }

    pub fn contract_id(&self) -> &ContractId {
        &self.contract_id
    }

    pub fn dataset_id(&self) -> DatasetId {
        self.dataset_id
    }

    pub fn quantity_basis(&self) -> QuantityUnit {
        self.quantity_basis
    }

    pub fn scenarios(&self) -> &[NormalizedScenario] {
        &self.scenarios
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioLayoutSpec {
    format_id: String,
    parser_version: String,
    ordered_scenario_ids: Vec<ScenarioId>,
}

impl ScenarioLayoutSpec {
    pub fn try_new(
        format_id: impl Into<String>,
        parser_version: impl Into<String>,
        ordered_scenario_ids: Vec<ScenarioId>,
    ) -> Result<Self, DomainError> {
        let format_id = format_id.into();
        let parser_version = parser_version.into();
        if format_id.trim().is_empty() || parser_version.trim().is_empty() {
            return Err(DomainError::MissingRequiredData {
                detail: "scenario layout requires a format id and parser version".to_owned(),
            });
        }
        if ordered_scenario_ids.is_empty() {
            return Err(DomainError::MissingRequiredData {
                detail: "scenario layout requires at least one scenario id".to_owned(),
            });
        }
        Ok(Self {
            format_id,
            parser_version,
            ordered_scenario_ids,
        })
    }

    pub fn format_id(&self) -> &str {
        &self.format_id
    }

    pub fn parser_version(&self) -> &str {
        &self.parser_version
    }

    pub fn ordered_scenario_ids(&self) -> &[ScenarioId] {
        &self.ordered_scenario_ids
    }
}

/// Registered scenario orders. The default registry is empty: no SPAN layout is assumed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScenarioLayoutRegistry {
    specs: Vec<ScenarioLayoutSpec>,
}

impl ScenarioLayoutRegistry {
    pub fn empty() -> Self {
        Self { specs: Vec::new() }
    }

    pub fn register(&mut self, spec: ScenarioLayoutSpec) -> Result<(), DomainError> {
        if self.specs.iter().any(|existing| {
            existing.format_id == spec.format_id && existing.parser_version == spec.parser_version
        }) {
            return Err(DomainError::AmbiguousDataset {
                detail: format!(
                    "scenario layout {} {} is already registered",
                    spec.format_id, spec.parser_version
                ),
            });
        }
        self.specs.push(spec);
        Ok(())
    }

    pub fn is_registered(&self, format_id: &str, parser_version: &str) -> bool {
        self.specs
            .iter()
            .any(|spec| spec.format_id == format_id && spec.parser_version == parser_version)
    }

    pub fn validate(
        &self,
        format_id: &str,
        parser_version: &str,
        array: &NormalizedRiskArray,
    ) -> Result<(), DomainError> {
        let Some(spec) = self
            .specs
            .iter()
            .find(|spec| spec.format_id == format_id && spec.parser_version == parser_version)
        else {
            return Err(DomainError::MissingRequiredData {
                detail: format!(
                    "no scenario layout specification is registered for format `{format_id}` parser `{parser_version}`"
                ),
            });
        };
        let actual: Vec<&ScenarioId> = array
            .scenarios
            .iter()
            .map(|scenario| &scenario.scenario_id)
            .collect();
        let expected: Vec<&ScenarioId> = spec.ordered_scenario_ids.iter().collect();
        if actual != expected {
            return Err(DomainError::RiskArrayRejected {
                detail: "scenario count or order does not match the registered layout".to_owned(),
            });
        }
        Ok(())
    }
}
