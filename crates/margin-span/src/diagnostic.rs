use crate::SpanError;
use margin_domain::{
    Currency, DomainError, Money, NormalizedBook, NormalizedRiskArray, ScenarioId,
};

pub const DIAGNOSTIC_LIMITATION: &str =
    "Diagnostic scenario aggregation is not an NSE Clearing or ICCL SPAN margin.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioDiagnostic {
    pub scenario_id: ScenarioId,
    pub portfolio_pnl: Money,
    pub portfolio_loss: Money,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticAggregation {
    pub scenario_results: Vec<ScenarioDiagnostic>,
    pub worst_loss: Option<Money>,
    pub limitation: &'static str,
}

/// Aggregates normalized per-unit P&L.
///
/// For each scenario `s`, `P_s = sum(q_i * R_i,s)` and `L_s = -P_s`.
/// The worst-loss diagnostic is `max(0, max L_s)` when the book is non-empty.
/// An empty book returns no amount.
///
/// All non-zero positions must belong to one exchange and one clearing corporation.
/// Cross-exchange netting is rejected here.
pub fn aggregate_scenarios(
    book: &NormalizedBook,
    arrays: &[NormalizedRiskArray],
) -> Result<DiagnosticAggregation, SpanError> {
    if book.positions().is_empty() {
        return Ok(DiagnosticAggregation {
            scenario_results: Vec::new(),
            worst_loss: None,
            limitation: DIAGNOSTIC_LIMITATION,
        });
    }
    let _scope = book.single_scope()?;

    let mut scenario_ids: Option<Vec<ScenarioId>> = None;
    let mut totals: Vec<Money> = Vec::new();
    for position in book.positions() {
        let Some(array) = arrays
            .iter()
            .find(|array| array.contract_id() == position.contract_id())
        else {
            return Err(SpanError::Domain(DomainError::MissingRequiredData {
                detail: format!("missing risk array for contract {}", position.contract_id()),
            }));
        };
        if array.quantity_basis() != position.quantity().unit() {
            return Err(SpanError::Domain(DomainError::QuantityUnitMismatch {
                left: position.quantity().unit(),
                right: array.quantity_basis(),
            }));
        }
        let ids: Vec<ScenarioId> = array
            .scenarios()
            .iter()
            .map(|scenario| scenario.scenario_id().clone())
            .collect();
        match &scenario_ids {
            None => {
                totals = array
                    .scenarios()
                    .iter()
                    .map(|scenario| Money::zero(scenario.pnl_per_unit().currency()))
                    .collect();
                scenario_ids = Some(ids);
            }
            Some(expected) if expected != &ids => {
                return Err(SpanError::Domain(DomainError::RiskArrayRejected {
                    detail: format!(
                        "scenario ids or order for contract {} do not match the rest of the portfolio",
                        position.contract_id()
                    ),
                }));
            }
            Some(_) => {}
        }
        for (index, scenario) in array.scenarios().iter().enumerate() {
            let contribution = scenario
                .pnl_per_unit()
                .checked_mul(position.quantity().amount())?;
            let current = totals[index];
            if current.currency() != contribution.currency() {
                return Err(SpanError::Domain(DomainError::CurrencyMismatch {
                    left: current.currency(),
                    right: contribution.currency(),
                }));
            }
            totals[index] = current.checked_add(contribution)?;
        }
    }

    let Some(ids) = scenario_ids else {
        return Err(SpanError::Domain(DomainError::MissingRequiredData {
            detail: "non-empty book produced no scenarios".to_owned(),
        }));
    };
    let mut scenario_results = Vec::with_capacity(ids.len());
    let mut worst = Money::zero(Currency::Inr);
    let mut worst_set = false;
    for (scenario_id, portfolio_pnl) in ids.into_iter().zip(totals) {
        let portfolio_loss = portfolio_pnl.checked_neg()?;
        if !worst_set {
            worst = Money::zero(portfolio_loss.currency());
            worst_set = true;
        }
        if portfolio_loss.amount() > worst.amount() {
            worst = portfolio_loss;
        }
        scenario_results.push(ScenarioDiagnostic {
            scenario_id,
            portfolio_pnl,
            portfolio_loss,
        });
    }
    Ok(DiagnosticAggregation {
        scenario_results,
        worst_loss: Some(worst),
        limitation: DIAGNOSTIC_LIMITATION,
    })
}
