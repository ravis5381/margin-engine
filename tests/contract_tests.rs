mod common;

use chrono::NaiveDate;
use common::{index_future, index_option, position, trade_date};
use margin_domain::{
    Contract, ContractDraft, ContractId, Currency, Exchange, InstrumentType, Money,
    OffsetEligibility, QuantityConversion, QuantityUnit, SignedQuantity, SourceContractId,
    assess_cross_exchange_offset,
};
use margin_rules::{
    InstrumentScope, MarginRule, MarginRuleRegistry, PolicyInputs, RuleQuery, RulesError,
    apply_margin_policy, select_rule,
};
use proptest::prelude::*;
use rust_decimal::Decimal;

#[test]
fn contract_identity_is_not_the_underlying_symbol() {
    let future = index_future();
    let option = index_option();
    assert_ne!(future.id(), option.id());
    assert_eq!(future.underlying().as_str(), "NIFTY");
    assert_eq!(future.source_contract_id().as_str(), "SRC-NIFTY-FUT");
    assert!(future.strike().is_none());
    assert_eq!(option.option_type().expect("call").code(), "CALL");
}

#[test]
fn futures_cannot_carry_strikes_and_unknown_products_are_rejected() {
    let draft = ContractDraft {
        id: ContractId::parse("SYN-BAD").expect("id"),
        exchange: Exchange::Nse,
        clearing_corporation: margin_domain::ClearingCorporation::Nscc,
        underlying: margin_domain::Underlying::parse("NIFTY").expect("underlying"),
        instrument_type: InstrumentType::IndexFuture,
        expiry: trade_date(),
        strike: Some(Money::parse("1", Currency::Inr).expect("strike")),
        option_type: None,
        lot_size: margin_domain::LotSize::new(1).expect("lot"),
        multiplier: margin_domain::Multiplier::new(Decimal::ONE).expect("multiplier"),
        currency: Currency::Inr,
        source_contract_id: SourceContractId::parse("SRC-BAD").expect("source"),
        effective_trading_date: trade_date(),
        contract_master_version: margin_domain::ContractMasterVersionId::from_uuid(
            uuid::Uuid::from_u128(9),
        ),
    };
    assert!(matches!(
        Contract::try_new(draft),
        Err(margin_domain::DomainError::InvalidContract { .. })
    ));
    assert!(matches!(
        InstrumentType::parse("CURRENCY_FUTURE"),
        Err(margin_domain::DomainError::UnsupportedProduct { .. })
    ));
}

#[test]
fn quantity_conversion_requires_an_explicit_factor() {
    let lots = SignedQuantity::new(Decimal::from(2), QuantityUnit::Lot);
    assert!(
        QuantityConversion::new(
            QuantityUnit::Lot,
            QuantityUnit::UnderlyingUnit,
            Decimal::from(65),
            "  "
        )
        .is_err()
    );
    let conversion = QuantityConversion::new(
        QuantityUnit::Lot,
        QuantityUnit::UnderlyingUnit,
        Decimal::from(65),
        "synthetic contract master v1",
    )
    .expect("conversion");
    let converted = conversion.apply(lots).expect("apply");
    assert_eq!(converted.unit(), QuantityUnit::UnderlyingUnit);
    assert_eq!(converted.amount(), Decimal::from(130));
    let future = index_future();
    assert_eq!(future.lot_size().get(), 65);
}

#[test]
fn duplicate_positions_sum_and_conflicts_do_not() {
    let first = position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        2,
        QuantityUnit::Lot,
    );
    let second = position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        3,
        QuantityUnit::Lot,
    );
    let book = margin_domain::NormalizedBook::from_positions(&[first, second]).expect("book");
    assert_eq!(book.positions().len(), 1);
    assert_eq!(book.positions()[0].quantity().amount(), Decimal::from(5));

    let base = position(
        "SYN-A",
        Exchange::Nse,
        margin_domain::ClearingCorporation::Nscc,
        1,
        QuantityUnit::Lot,
    );
    let other_origin = position_with_origin("desk-b");
    assert!(matches!(
        margin_domain::NormalizedBook::from_positions(&[base, other_origin]),
        Err(margin_domain::DomainError::ConflictingPositionAttributes { .. })
    ));
}

fn position_with_origin(origin: &str) -> margin_domain::Position {
    margin_domain::Position::try_new(margin_domain::PositionDraft {
        portfolio_id: margin_domain::PortfolioId::parse("PORTFOLIO-001").expect("portfolio"),
        aggregation_scope: common::scope(),
        contract_id: ContractId::parse("SYN-A").expect("contract"),
        exchange: Exchange::Nse,
        quantity: SignedQuantity::new(Decimal::ONE, QuantityUnit::Lot),
        average_trade_price: None,
        valuation: None,
        origin: origin.to_owned(),
        timestamp: common::at_hour(9),
        clearing_corporation: margin_domain::ClearingCorporation::Nscc,
        trading_date: trade_date(),
    })
    .expect("position")
}

#[test]
fn cross_exchange_offset_is_rejected_even_when_a_reference_is_present() {
    assert!(assess_cross_exchange_offset(&OffsetEligibility::NotProvided).is_err());
    assert!(
        assess_cross_exchange_offset(&OffsetEligibility::Configured {
            rule_reference: "hypothetical interoperability note".to_owned(),
        })
        .is_err()
    );
}

#[test]
fn effective_rule_selection_does_not_calculate_a_charge() {
    let mut registry = MarginRuleRegistry::empty();
    let on = trade_date();
    let query = RuleQuery {
        clearing_corporation: margin_domain::ClearingCorporation::Iccl,
        exchange: Exchange::Bse,
        product: InstrumentType::IndexOption,
        on,
    };
    assert!(select_rule(&registry, &query).is_err());
    registry
        .register(
            MarginRule::try_new(
                margin_domain::RuleVersionId::parse("ICCL-ELM-2026").expect("rule"),
                margin_domain::ClearingCorporation::Iccl,
                Some(Exchange::Bse),
                InstrumentScope::Instrument(InstrumentType::IndexOption),
                NaiveDate::from_ymd_opt(2026, 1, 1).expect("from"),
                None,
                "unverified ICCL circular placeholder",
            )
            .expect("rule"),
        )
        .expect("register");
    let rule = select_rule(&registry, &query).expect("selected");
    let normalized = common::book(&[position(
        "SYN-SENSEX-OPT",
        Exchange::Bse,
        margin_domain::ClearingCorporation::Iccl,
        1,
        QuantityUnit::Lot,
    )]);
    let inputs = PolicyInputs { book: &normalized };
    assert!(matches!(
        apply_margin_policy(rule, &inputs),
        Err(RulesError::SpecificationRequired { .. })
    ));
    let before = RuleQuery {
        on: NaiveDate::from_ymd_opt(2025, 12, 31).expect("before"),
        ..query
    };
    assert!(select_rule(&registry, &before).is_err());
}

proptest! {
    #[test]
    fn money_addition_is_commutative_and_exact(left in -1_000_000i64..1_000_000, right in -1_000_000i64..1_000_000) {
        let left_money = Money::from_minor_units(left, 4, Currency::Inr).expect("left");
        let right_money = Money::from_minor_units(right, 4, Currency::Inr).expect("right");
        prop_assert_eq!(
            left_money.checked_add(right_money).expect("sum"),
            right_money.checked_add(left_money).expect("sum")
        );
        let zero = Money::zero(Currency::Inr);
        prop_assert_eq!(left_money.checked_add(zero).expect("zero"), left_money);
    }
}
