use margin_domain::{CalculationId, DatasetId, DomainError, Money, RuleVersionId};
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReconciliationError {
    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("invalid reconciliation input: {detail}")]
    InvalidInput { detail: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticCategory {
    WrongContractMapping,
    MissingRiskArray,
    IncorrectQuantityNormalization,
    ScenarioSignOrOrderMismatch,
    IncorrectSpreadEligibility,
    IncorrectOptionValuation,
    ElmMismatch,
    DeliveryMarginMismatch,
    IncorrectAggregationScope,
    IncorrectRounding,
    OutdatedOrWrongSnapshot,
    UnsupportedOrMissingExchangeRule,
}

impl DiagnosticCategory {
    pub const fn code(self) -> &'static str {
        match self {
            Self::WrongContractMapping => "wrong_contract_mapping",
            Self::MissingRiskArray => "missing_risk_array",
            Self::IncorrectQuantityNormalization => "incorrect_quantity_normalization",
            Self::ScenarioSignOrOrderMismatch => "scenario_sign_or_order_mismatch",
            Self::IncorrectSpreadEligibility => "incorrect_spread_eligibility",
            Self::IncorrectOptionValuation => "incorrect_option_valuation",
            Self::ElmMismatch => "elm_mismatch",
            Self::DeliveryMarginMismatch => "delivery_margin_mismatch",
            Self::IncorrectAggregationScope => "incorrect_aggregation_scope",
            Self::IncorrectRounding => "incorrect_rounding",
            Self::OutdatedOrWrongSnapshot => "outdated_or_wrong_snapshot",
            Self::UnsupportedOrMissingExchangeRule => "unsupported_or_missing_exchange_rule",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionStatus {
    Unresolved,
    Explained { note: String },
    AcceptedException { reference: String },
}

impl ResolutionStatus {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::Explained { .. } => "explained",
            Self::AcceptedException { .. } => "accepted_exception",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tolerance {
    absolute: Money,
    relative: Decimal,
}

impl Tolerance {
    pub fn new(absolute: Money, relative: Decimal) -> Result<Self, ReconciliationError> {
        if absolute.is_negative() || relative.is_sign_negative() {
            return Err(ReconciliationError::InvalidInput {
                detail: "tolerance bounds cannot be negative".to_owned(),
            });
        }
        Ok(Self { absolute, relative })
    }

    pub fn absolute(&self) -> Money {
        self.absolute
    }

    pub fn relative(&self) -> Decimal {
        self.relative
    }
}

pub struct CompareInput {
    pub expected: Money,
    pub calculated: Money,
    pub tolerance: Tolerance,
    pub category: Option<DiagnosticCategory>,
    pub position_refs: Vec<String>,
    pub risk_dataset_id: Option<DatasetId>,
    pub calculation_id: Option<CalculationId>,
    pub rule_version_id: Option<RuleVersionId>,
    pub calculation_trace: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    expected: Money,
    calculated: Money,
    absolute: Money,
    relative: Option<Decimal>,
    material: bool,
    within_diagnostic_tolerance: bool,
    category: Option<DiagnosticCategory>,
    resolution: ResolutionStatus,
    position_refs: Vec<String>,
    risk_dataset_id: Option<DatasetId>,
    calculation_id: Option<CalculationId>,
    rule_version_id: Option<RuleVersionId>,
    calculation_trace: String,
}

impl Difference {
    pub fn expected(&self) -> Money {
        self.expected
    }

    pub fn calculated(&self) -> Money {
        self.calculated
    }

    pub fn absolute(&self) -> Money {
        self.absolute
    }

    pub fn relative(&self) -> Option<Decimal> {
        self.relative
    }

    pub fn material(&self) -> bool {
        self.material
    }

    pub fn within_diagnostic_tolerance(&self) -> bool {
        self.within_diagnostic_tolerance
    }

    pub fn category(&self) -> Option<DiagnosticCategory> {
        self.category
    }

    pub fn resolution(&self) -> &ResolutionStatus {
        &self.resolution
    }

    pub fn position_refs(&self) -> &[String] {
        &self.position_refs
    }

    pub fn risk_dataset_id(&self) -> Option<DatasetId> {
        self.risk_dataset_id
    }

    pub fn calculation_id(&self) -> Option<CalculationId> {
        self.calculation_id
    }

    pub fn rule_version_id(&self) -> Option<&RuleVersionId> {
        self.rule_version_id.as_ref()
    }

    pub fn calculation_trace(&self) -> &str {
        &self.calculation_trace
    }

    pub fn mark_explained(mut self, note: impl Into<String>) -> Result<Self, ReconciliationError> {
        self.set_resolution(true, note.into())
    }

    pub fn mark_accepted_exception(
        mut self,
        reference: impl Into<String>,
    ) -> Result<Self, ReconciliationError> {
        self.set_resolution(false, reference.into())
    }

    fn set_resolution(
        &mut self,
        explained: bool,
        note: String,
    ) -> Result<Self, ReconciliationError> {
        if !matches!(self.resolution, ResolutionStatus::Unresolved) {
            return Err(ReconciliationError::InvalidInput {
                detail: "resolution status cannot be rewritten".to_owned(),
            });
        }
        if note.trim().is_empty() {
            return Err(ReconciliationError::InvalidInput {
                detail: "resolution requires a note or reference".to_owned(),
            });
        }
        self.resolution = if explained {
            ResolutionStatus::Explained { note }
        } else {
            ResolutionStatus::AcceptedException { reference: note }
        };
        Ok(self.clone())
    }
}

pub fn compare(input: CompareInput) -> Result<Difference, ReconciliationError> {
    if input.calculation_trace.trim().is_empty() {
        return Err(ReconciliationError::InvalidInput {
            detail: "reconciliation requires a calculation trace".to_owned(),
        });
    }
    if input.expected.currency() != input.tolerance.absolute().currency() {
        return Err(ReconciliationError::Domain(DomainError::CurrencyMismatch {
            left: input.expected.currency(),
            right: input.tolerance.absolute().currency(),
        }));
    }
    let signed = input.calculated.checked_sub(input.expected)?;
    let absolute = if signed.is_negative() {
        signed.checked_neg()?
    } else {
        signed
    };
    let relative = if input.expected.is_zero() {
        None
    } else {
        Some(
            absolute
                .amount()
                .checked_div(input.expected.amount().abs())
                .ok_or(DomainError::MonetaryOverflow)?,
        )
    };
    let material = !absolute.is_zero();
    let within_absolute = absolute.amount().abs() <= input.tolerance.absolute().amount();
    let within_relative = match relative {
        Some(value) => value <= input.tolerance.relative(),
        None => !material,
    };
    Ok(Difference {
        expected: input.expected,
        calculated: input.calculated,
        absolute,
        relative,
        material,
        within_diagnostic_tolerance: within_absolute && within_relative,
        category: input.category,
        resolution: ResolutionStatus::Unresolved,
        position_refs: input.position_refs,
        risk_dataset_id: input.risk_dataset_id,
        calculation_id: input.calculation_id,
        rule_version_id: input.rule_version_id,
        calculation_trace: input.calculation_trace,
    })
}
