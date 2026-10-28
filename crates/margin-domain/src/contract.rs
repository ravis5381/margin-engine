use crate::clearing::{ClearingCorporation, Exchange};
use crate::error::DomainError;
use crate::ids::{ContractId, ContractMasterVersionId, SourceContractId};
use crate::instrument::{InstrumentType, OptionType, Underlying};
use crate::money::{Currency, Money};
use chrono::NaiveDate;
use rust_decimal::Decimal;

/// Positive exchange lot size. It is not applied as a hidden quantity conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LotSize(u32);

impl LotSize {
    pub fn new(value: u32) -> Result<Self, DomainError> {
        if value == 0 {
            return Err(DomainError::InvalidContract {
                detail: "lot size must be positive".to_owned(),
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Positive contract multiplier. It is not applied unless a conversion names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Multiplier(Decimal);

impl Multiplier {
    pub fn new(value: Decimal) -> Result<Self, DomainError> {
        if value <= Decimal::ZERO {
            return Err(DomainError::InvalidContract {
                detail: "contract multiplier must be positive".to_owned(),
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> Decimal {
        self.0
    }
}

/// Inputs for [`Contract::try_new`]. Fields are validated together so an option cannot
/// omit its strike and a future cannot carry one.
pub struct ContractDraft {
    pub id: ContractId,
    pub exchange: Exchange,
    pub clearing_corporation: ClearingCorporation,
    pub underlying: Underlying,
    pub instrument_type: InstrumentType,
    pub expiry: NaiveDate,
    pub strike: Option<Money>,
    pub option_type: Option<OptionType>,
    pub lot_size: LotSize,
    pub multiplier: Multiplier,
    pub currency: Currency,
    pub source_contract_id: SourceContractId,
    pub effective_trading_date: NaiveDate,
    pub contract_master_version: ContractMasterVersionId,
}

/// Canonical contract. Identity is [`ContractId`], never a display symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contract {
    id: ContractId,
    exchange: Exchange,
    clearing_corporation: ClearingCorporation,
    underlying: Underlying,
    instrument_type: InstrumentType,
    expiry: NaiveDate,
    strike: Option<Money>,
    option_type: Option<OptionType>,
    lot_size: LotSize,
    multiplier: Multiplier,
    currency: Currency,
    source_contract_id: SourceContractId,
    effective_trading_date: NaiveDate,
    contract_master_version: ContractMasterVersionId,
}

impl Contract {
    pub fn try_new(draft: ContractDraft) -> Result<Self, DomainError> {
        if draft.currency != Currency::Inr {
            return Err(DomainError::InvalidContract {
                detail: "only INR contracts are supported".to_owned(),
            });
        }
        match (
            draft.instrument_type.is_option(),
            draft.strike,
            draft.option_type,
        ) {
            (true, Some(strike), Some(option_type)) => {
                if strike.currency() != draft.currency {
                    return Err(DomainError::CurrencyMismatch {
                        left: strike.currency(),
                        right: draft.currency,
                    });
                }
                if strike.is_negative() {
                    return Err(DomainError::InvalidContract {
                        detail: "option strike cannot be negative".to_owned(),
                    });
                }
                Ok(Self::from_draft(draft, Some(strike), Some(option_type)))
            }
            (true, _, _) => Err(DomainError::InvalidContract {
                detail: "option contracts require a strike and an option type".to_owned(),
            }),
            (false, None, None) => Ok(Self::from_draft(draft, None, None)),
            (false, _, _) => Err(DomainError::InvalidContract {
                detail: "future contracts cannot carry a strike or option type".to_owned(),
            }),
        }
    }

    fn from_draft(
        draft: ContractDraft,
        strike: Option<Money>,
        option_type: Option<OptionType>,
    ) -> Self {
        Self {
            id: draft.id,
            exchange: draft.exchange,
            clearing_corporation: draft.clearing_corporation,
            underlying: draft.underlying,
            instrument_type: draft.instrument_type,
            expiry: draft.expiry,
            strike,
            option_type,
            lot_size: draft.lot_size,
            multiplier: draft.multiplier,
            currency: draft.currency,
            source_contract_id: draft.source_contract_id,
            effective_trading_date: draft.effective_trading_date,
            contract_master_version: draft.contract_master_version,
        }
    }

    pub fn id(&self) -> &ContractId {
        &self.id
    }

    pub fn exchange(&self) -> Exchange {
        self.exchange
    }

    pub fn clearing_corporation(&self) -> ClearingCorporation {
        self.clearing_corporation
    }

    pub fn underlying(&self) -> &Underlying {
        &self.underlying
    }

    pub fn instrument_type(&self) -> InstrumentType {
        self.instrument_type
    }

    pub fn expiry(&self) -> NaiveDate {
        self.expiry
    }

    pub fn strike(&self) -> Option<Money> {
        self.strike
    }

    pub fn option_type(&self) -> Option<OptionType> {
        self.option_type
    }

    pub fn lot_size(&self) -> LotSize {
        self.lot_size
    }

    pub fn multiplier(&self) -> Multiplier {
        self.multiplier
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    pub fn source_contract_id(&self) -> &SourceContractId {
        &self.source_contract_id
    }

    pub fn effective_trading_date(&self) -> NaiveDate {
        self.effective_trading_date
    }

    pub fn contract_master_version(&self) -> ContractMasterVersionId {
        self.contract_master_version
    }
}
