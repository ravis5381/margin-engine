//! Margin policy above SPAN.
//!
//! Rule metadata can be registered, but no component calculator is implemented.
//! Selecting a rule never produces an amount.

mod policy;

pub use policy::{
    InstrumentScope, MarginRule, MarginRuleRegistry, PolicyInputs, RuleQuery, RulesError,
    additional_margin, apply_margin_policy, cross_margin_benefit, crystallised_obligations,
    delivery_margin, elm, premium_obligation, select_rule,
};
