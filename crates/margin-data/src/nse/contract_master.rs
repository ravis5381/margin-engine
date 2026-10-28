//! NSE F&O contract master `NSE_FO_contract_DDMMYYYY.csv.gz`.
//!
//! The file is published at `nsearchives.nseindia.com/content/fo/`. Expiry is
//! seconds since 1980-01-01 at 14:30:00 UTC. Strike is stored in paise. `MinLot` is the lot
//! size. A deleted row (`DelFlg` other than `N`) is skipped. The lot size is
//! not applied as a quantity conversion.

use chrono::{Duration, NaiveDate};
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::io::Read;
use std::str::FromStr;

const UNCOMPRESSED_LIMIT: u64 = 104_857_600;
const REQUIRED: [&str; 8] = [
    "FinInstrmNm",
    "TckrSymb",
    "XpryDt",
    "StrkPric",
    "OptnTp",
    "MinLot",
    "NewBrdLotQty",
    "DelFlg",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContractMaster {
    lots: BTreeMap<ContractKey, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ContractKey {
    product: String,
    expiry: NaiveDate,
    instrument: &'static str,
    strike: Option<Decimal>,
}

impl ContractMaster {
    pub fn is_empty(&self) -> bool {
        self.lots.is_empty()
    }

    pub fn parse_gzip(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
            return Err("contract master is not a gzip payload".to_owned());
        }
        let mut decoder = GzDecoder::new(bytes);
        let mut stored = Vec::new();
        let mut buffer = [0_u8; 8192];
        let mut total = 0_u64;
        loop {
            let read = decoder
                .read(&mut buffer)
                .map_err(|error| format!("contract master gzip could not be read: {error}"))?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            if total > UNCOMPRESSED_LIMIT {
                return Err("contract master uncompressed size exceeds the limit".to_owned());
            }
            stored.extend_from_slice(&buffer[..read]);
        }
        let text =
            String::from_utf8(stored).map_err(|_| "contract master is not UTF-8".to_owned())?;
        Self::parse_csv(&text)
    }

    pub fn lot_for(
        &self,
        instrument: &str,
        product: &str,
        expiry: Option<NaiveDate>,
        strike: Option<Decimal>,
    ) -> Result<Option<u32>, String> {
        if self.lots.is_empty() || instrument == "stock" {
            return Ok(None);
        }
        let Some(expiry) = expiry else {
            return Err(format!(
                "{product} {instrument} has no expiry in the contract master"
            ));
        };
        let key = ContractKey {
            product: product.to_owned(),
            expiry,
            instrument: match instrument {
                "future" => "future",
                "call" => "call",
                "put" => "put",
                other => return Err(format!("{other} is not a contract-master instrument")),
            },
            strike,
        };
        self.lots.get(&key).copied().map(Some).ok_or_else(|| {
            format!(
                "{product} {instrument} {expiry} strike {strike:?} is not in the contract master"
            )
        })
    }

    fn parse_csv(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let header = lines
            .next()
            .ok_or_else(|| "contract master has no header".to_owned())?;
        let columns: Vec<&str> = header.split(',').collect();
        let index = |name: &str| {
            columns
                .iter()
                .position(|column| *column == name)
                .ok_or_else(|| format!("contract master header has no {name} column"))
        };
        let mut indexes = [0usize; REQUIRED.len()];
        for (slot, name) in REQUIRED.iter().enumerate() {
            indexes[slot] = index(name)?;
        }
        let width = columns.len();
        let mut lots = BTreeMap::new();
        for (row_number, line) in lines.enumerate() {
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() != width {
                return Err(format!(
                    "contract master row {} has {} columns and the header has {width}",
                    row_number + 2,
                    fields.len()
                ));
            }
            let field = |name_index: usize| fields[indexes[name_index]].trim();
            if field(7) != "N" || field(0).is_empty() {
                continue;
            }
            let instrument = contract_instrument(field(0), field(4))?;
            let product = field(1);
            if !product_code(product) {
                return Err(format!(
                    "contract master row {} has no product code",
                    row_number + 2
                ));
            }
            let expiry = nse_expiry(field(2))?;
            let strike = contract_strike(instrument, field(3))?;
            let lot = lot_size(field(5), field(6))?;
            let key = ContractKey {
                product: product.to_owned(),
                expiry,
                instrument,
                strike,
            };
            if let Some(existing) = lots.insert(key, lot)
                && existing != lot
            {
                return Err(format!(
                    "contract master repeats {product} {instrument} {expiry} with two lot sizes"
                ));
            }
        }
        if lots.is_empty() {
            return Err("contract master has no active contracts".to_owned());
        }
        Ok(Self { lots })
    }
}

fn product_code(product: &str) -> bool {
    !product.is_empty()
        && product.len() <= 20
        && product.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '&'
        })
}

fn contract_instrument(name: &str, option_type: &str) -> Result<&'static str, String> {
    match (name, option_type) {
        ("FUTIDX" | "FUTSTK", "XX") => Ok("future"),
        ("OPTIDX" | "OPTSTK", "CE") => Ok("call"),
        ("OPTIDX" | "OPTSTK", "PE") => Ok("put"),
        _ => Err(format!(
            "contract master instrument {name} option type {option_type} is not recognized"
        )),
    }
}

fn contract_strike(instrument: &str, text: &str) -> Result<Option<Decimal>, String> {
    if instrument == "future" {
        if text != "-1" {
            return Err(format!("contract master future strike is {text}"));
        }
        return Ok(None);
    }
    let raw = Decimal::from_str(text)
        .map_err(|_| format!("contract master strike {text} is not a decimal"))?;
    let strike = raw
        .checked_div(Decimal::from(100))
        .ok_or_else(|| "contract master strike overflowed".to_owned())?;
    if strike <= Decimal::ZERO {
        return Err(format!(
            "contract master option strike {text} is not positive"
        ));
    }
    Ok(Some(strike.normalize()))
}

fn lot_size(minimum: &str, board: &str) -> Result<u32, String> {
    if minimum != board {
        return Err(format!(
            "contract master minimum lot {minimum} does not match the board lot {board}"
        ));
    }
    let lot = minimum
        .parse::<u32>()
        .map_err(|_| format!("contract master lot {minimum} is not a whole number"))?;
    if lot == 0 {
        return Err("contract master lot size is zero".to_owned());
    }
    Ok(lot)
}

fn nse_expiry(text: &str) -> Result<NaiveDate, String> {
    let seconds = text
        .parse::<i64>()
        .map_err(|_| format!("contract master expiry {text} is not a second count"))?;
    // Every active row in NSE_FO_contract_28102026.csv.gz is 14:30:00 UTC.
    const EXPIRY_OFFSET: i64 = 52_200;
    if seconds < EXPIRY_OFFSET || (seconds - EXPIRY_OFFSET) % 86_400 != 0 {
        return Err(format!("contract master expiry {text} is not 14:30:00 UTC"));
    }
    NaiveDate::from_ymd_opt(1980, 1, 1)
        .expect("1980-01-01")
        .checked_add_signed(Duration::days((seconds - EXPIRY_OFFSET) / 86_400))
        .ok_or_else(|| format!("contract master expiry {text} is outside the calendar"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn gzip(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).expect("write");
        encoder.finish().expect("gzip")
    }

    #[test]
    fn the_observed_contract_row_supplies_the_lot_size() {
        let header = "FinInstrmId,UndrlygFinInstrmId,FinInstrmNm,TckrSymb,XpryDt,StrkPric,OptnTp,PrtdToTrad,MinLot,NewBrdLotQty,DelFlg";
        let future = "1,2,FUTIDX,NIFTY,1479911400,-1,XX,1,65,65,N";
        let stock = "1,2,OPTSTK,BAJAJ-AUTO,1479911400,680000,CE,1,75,75,N";
        let deleted = "1,2,FUTIDX,NIFTY,1479911400,-1,XX,1,1,1,Y";
        let master =
            ContractMaster::parse_gzip(&gzip(&format!("{header}\n{future}\n{stock}\n{deleted}\n")))
                .expect("master");
        assert_eq!(
            master
                .lot_for(
                    "call",
                    "BAJAJ-AUTO",
                    Some(NaiveDate::from_ymd_opt(2026, 11, 23).expect("date")),
                    Some(Decimal::from(6800))
                )
                .expect("stock lot"),
            Some(75)
        );
        let expiry = NaiveDate::from_ymd_opt(2026, 11, 23).expect("date");
        assert_eq!(
            master
                .lot_for("future", "NIFTY", Some(expiry), None)
                .expect("lot"),
            Some(65)
        );
        assert!(
            master
                .lot_for(
                    "future",
                    "NIFTY",
                    Some(expiry.succ_opt().expect("next")),
                    None
                )
                .is_err()
        );
    }
}
