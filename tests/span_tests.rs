mod common;

use common::{book, pnl_array, position};
use margin_domain::{
    Currency, DatasetId, DomainError, Exchange, Money, QuantityUnit, ScenarioId,
    ScenarioLayoutRegistry, ScenarioLayoutSpec, ValueConvention,
};
use margin_span::{
    DIAGNOSTIC_LIMITATION, SpanError, aggregate_scenarios, diagnostic_is_not_scanning_risk,
    net_option_value, scanning_risk, short_option_minimum, spread_charge,
};
use proptest::prelude::*;
use rust_decimal::Decimal;

#[test]
fn diagnostic_worst_loss_follows_the_specified_formula() {
    let dataset_id = DatasetId::new();
    let long = position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        2,
        QuantityUnit::Lot,
    );
    let short = position(
        "SYN-B",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        -1,
        QuantityUnit::Lot,
    );
    let normalized = book(&[long, short]);
    let arrays = [
        pnl_array(
            "SYN-A",
            dataset_id,
            &[("s1", "10"), ("s2", "-4")],
            QuantityUnit::Lot,
        ),
        pnl_array(
            "SYN-B",
            dataset_id,
            &[("s1", "10"), ("s2", "-4")],
            QuantityUnit::Lot,
        ),
    ];
    let diagnostic = aggregate_scenarios(&normalized, &arrays).expect("diagnostic");
    assert_eq!(diagnostic.limitation, DIAGNOSTIC_LIMITATION);
    assert!(diagnostic_is_not_scanning_risk(&diagnostic));
    assert_eq!(
        diagnostic.scenario_results[0].portfolio_pnl,
        Money::parse("10", Currency::Inr).expect("pnl")
    );
    assert_eq!(
        diagnostic.scenario_results[1].portfolio_loss,
        Money::parse("4", Currency::Inr).expect("loss")
    );
    assert_eq!(
        diagnostic.worst_loss.expect("worst"),
        Money::parse("4", Currency::Inr).expect("worst")
    );
}

#[test]
fn loss_convention_and_scale_are_applied_once() {
    let dataset_id = DatasetId::new();
    let array = margin_domain::NormalizedRiskArray::try_new(margin_domain::RiskArrayDraft {
        contract_id: margin_domain::ContractId::parse("SYN-C").expect("id"),
        dataset_id,
        quantity_basis: QuantityUnit::Lot,
        scenarios: vec![margin_domain::ScenarioValueDraft {
            scenario_id: ScenarioId::parse("loss-1").expect("scenario"),
            official_index: 0,
            amount: Money::parse("5", Currency::Inr).expect("amount"),
            convention: ValueConvention::Loss,
            quantity_basis: QuantityUnit::Lot,
            scale: Decimal::from(10),
            source_raw: "5".to_owned(),
            source_locator: None,
        }],
    })
    .expect("array");
    assert_eq!(array.scenarios()[0].source_raw(), "5");
    assert_eq!(
        array.scenarios()[0].pnl_per_unit(),
        Money::parse("-50", Currency::Inr).expect("pnl")
    );
    let normalized = book(&[position(
        "SYN-C",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        1,
        QuantityUnit::Lot,
    )]);
    let diagnostic = aggregate_scenarios(&normalized, &[array]).expect("diagnostic");
    assert_eq!(
        diagnostic.worst_loss.expect("worst"),
        Money::parse("50", Currency::Inr).expect("loss")
    );
}

#[test]
fn missing_or_inconsistent_arrays_and_cross_exchange_books_fail() {
    let dataset_id = DatasetId::new();
    let nse = position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        1,
        QuantityUnit::Lot,
    );
    let normalized = book(std::slice::from_ref(&nse));
    assert!(matches!(
        aggregate_scenarios(&normalized, &[]),
        Err(SpanError::Domain(DomainError::MissingRequiredData { .. }))
    ));
    let zero = position(
        "SYN-MISSING",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        0,
        QuantityUnit::Lot,
    );
    let with_zero = book(&[nse.clone(), zero]);
    let array = pnl_array("SYN-A", dataset_id, &[("s1", "3")], QuantityUnit::Lot);
    assert!(aggregate_scenarios(&with_zero, &[array]).is_ok());

    let mismatched = pnl_array("SYN-A", dataset_id, &[("other", "3")], QuantityUnit::Lot);
    let expected = pnl_array("SYN-B", dataset_id, &[("s1", "3")], QuantityUnit::Lot);
    let two = book(&[
        nse,
        position(
            "SYN-B",
            Exchange::Nse,
            margin_domain::ClearingCorporation::Nscc,
            1,
            QuantityUnit::Lot,
        ),
    ]);
    assert!(aggregate_scenarios(&two, &[mismatched, expected]).is_err());

    let cross = [
        position(
            "SYN-A",
            Exchange::Nse,
            margin_domain::ClearingCorporation::Nscc,
            1,
            QuantityUnit::Lot,
        ),
        position(
            "SYN-B",
            Exchange::Bse,
            margin_domain::ClearingCorporation::Iccl,
            -1,
            QuantityUnit::Lot,
        ),
    ];
    assert!(
        margin_domain::NormalizedBook::from_positions(&cross)
            .expect("normalized")
            .single_scope()
            .is_err()
    );
}

#[test]
fn quantity_units_are_not_converted_from_lot_size() {
    let dataset_id = DatasetId::new();
    let normalized = book(&[position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        1,
        QuantityUnit::Lot,
    )]);
    let array = pnl_array(
        "SYN-A",
        dataset_id,
        &[("s1", "1")],
        QuantityUnit::UnderlyingUnit,
    );
    assert!(matches!(
        aggregate_scenarios(&normalized, &[array]),
        Err(SpanError::Domain(DomainError::QuantityUnitMismatch { .. }))
    ));
}

#[test]
fn official_span_components_do_not_return_amounts() {
    let normalized = book(&[position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        4,
        QuantityUnit::Lot,
    )]);
    assert!(matches!(
        scanning_risk(&ScenarioLayoutRegistry::empty(), "span", "v0", &normalized),
        Err(SpanError::Domain(DomainError::MissingRequiredData { .. }))
    ));
    let mut registry = ScenarioLayoutRegistry::empty();
    registry
        .register(
            ScenarioLayoutSpec::try_new(
                "span",
                "v0",
                vec![ScenarioId::parse("s1").expect("scenario")],
            )
            .expect("spec"),
        )
        .expect("register");
    assert!(matches!(
        scanning_risk(&registry, "span", "v0", &normalized),
        Err(SpanError::SpecificationRequired {
            component: "scanning_risk",
            ..
        })
    ));
    assert!(spread_charge(&normalized).is_err());
    assert!(short_option_minimum(&normalized).is_err());
    assert!(net_option_value(&normalized).is_err());
}

#[test]
fn an_all_profit_book_has_zero_diagnostic_worst_loss_not_a_span_margin() {
    let dataset_id = DatasetId::new();
    let normalized = book(&[position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        1,
        QuantityUnit::Lot,
    )]);
    let array = pnl_array(
        "SYN-A",
        dataset_id,
        &[("s1", "5"), ("s2", "7")],
        QuantityUnit::Lot,
    );
    let diagnostic = aggregate_scenarios(&normalized, &[array]).expect("diagnostic");
    assert_eq!(
        diagnostic.worst_loss.expect("worst"),
        Money::zero(Currency::Inr)
    );
    assert!(scanning_risk(&ScenarioLayoutRegistry::empty(), "span", "v0", &normalized).is_err());
}

proptest! {
    #[test]
    fn reordering_and_zero_quantity_do_not_change_the_diagnostic(
        first in -20i64..20,
        second in -20i64..20,
    ) {
        let dataset_id = DatasetId::new();
        let left = position("SYN-A", Exchange::Nse, margin_domain::ClearingCorporation::Nscc, first, QuantityUnit::Lot);
        let right = position("SYN-B", Exchange::Nse, margin_domain::ClearingCorporation::Nscc, second, QuantityUnit::Lot);
        let zero = position("SYN-Z", Exchange::Nse, margin_domain::ClearingCorporation::Nscc, 0, QuantityUnit::Lot);
        let forward = book(&[left.clone(), right.clone(), zero]);
        let backward = book(&[right, left]);
        prop_assert_eq!(forward.positions().len(), backward.positions().len());
        let arrays = [
            pnl_array("SYN-A", dataset_id, &[("s1", "2"), ("s2", "-3")], QuantityUnit::Lot),
            pnl_array("SYN-B", dataset_id, &[("s1", "2"), ("s2", "-3")], QuantityUnit::Lot),
        ];
        let forward_result = aggregate_scenarios(&forward, &arrays).expect("forward");
        let backward_result = aggregate_scenarios(&backward, &arrays).expect("backward");
        prop_assert_eq!(forward_result.worst_loss, backward_result.worst_loss);
        prop_assert_eq!(forward_result.scenario_results, backward_result.scenario_results);
    }
}
