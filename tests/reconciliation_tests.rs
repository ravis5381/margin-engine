use margin_domain::{Currency, Money};
use margin_reconciliation::{
    CompareInput, DiagnosticCategory, ResolutionStatus, Tolerance, compare,
};
use margin_rules::{PolicyInputs, cross_margin_benefit};
use rust_decimal::Decimal;

fn money(amount: &str) -> Money {
    Money::parse(amount, Currency::Inr).expect("money")
}

fn comparison(
    expected: &str,
    calculated: &str,
    absolute: &str,
    relative: &str,
) -> margin_reconciliation::Difference {
    compare(CompareInput {
        expected: money(expected),
        calculated: money(calculated),
        tolerance: Tolerance::new(
            money(absolute),
            Decimal::from_str_exact(relative).expect("relative"),
        )
        .expect("tolerance"),
        category: None,
        position_refs: vec!["SYN-A".to_owned()],
        risk_dataset_id: None,
        calculation_id: None,
        rule_version_id: None,
        calculation_trace: "synthetic comparison".to_owned(),
    })
    .expect("difference")
}

#[test]
fn tolerance_does_not_hide_a_material_difference() {
    let difference = comparison("100", "100.01", "0.05", "0.01");
    assert!(difference.material());
    assert!(difference.within_diagnostic_tolerance());
    assert!(difference.category().is_none());
    assert!(matches!(
        difference.resolution(),
        ResolutionStatus::Unresolved
    ));
    let explained = difference
        .mark_explained("price source differed in the reference file")
        .expect("explain");
    assert!(explained.material());
}

#[test]
fn a_zero_expected_amount_has_no_relative_difference() {
    let difference = comparison("0", "1", "10", "1");
    assert!(difference.relative().is_none());
    assert!(difference.material());
    assert!(!difference.within_diagnostic_tolerance());
}

#[test]
fn categories_are_not_inferred() {
    let mut input_difference = comparison("10", "12", "0", "0");
    assert!(input_difference.category().is_none());
    assert!(!input_difference.within_diagnostic_tolerance());
    let _ = DiagnosticCategory::ElmMismatch.code();
    input_difference = comparison("10", "10", "0", "0");
    assert!(!input_difference.material());
}

#[test]
fn cross_margin_flag_does_not_create_a_benefit() {
    let normalized = {
        // A local empty book is enough: the benefit function must not price it.
        margin_domain::NormalizedBook::from_positions(&[]).expect("empty")
    };
    let inputs = PolicyInputs { book: &normalized };
    assert!(cross_margin_benefit(true, &inputs).is_err());
    assert!(cross_margin_benefit(false, &inputs).is_err());
}
