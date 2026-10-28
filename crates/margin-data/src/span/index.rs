//! Derived SPAN index. The archived zip stays the source. This file is the
//! parsed contracts for that sha256, so a later quote does not walk the XML.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;

use super::quote::{
    CalendarSpread, Commodity, CommodityFamily, ContractKind, SpanContract, SpanFileIndex,
    SpreadFault, span_index_from_zip,
};

const MAGIC: &[u8; 8] = b"SPANIDX1";
const VERSION: u8 = 1;
const MAX_STRING: usize = 1_048_576;
const MAX_COUNT: u32 = 2_000_000;
const MEMORY_FILES: usize = 4;

pub(crate) struct SpanIndexCache {
    dir: PathBuf,
    memory: Mutex<Memory>,
}

struct Memory {
    order: VecDeque<String>,
    files: HashMap<String, Arc<SpanFileIndex>>,
}

impl SpanIndexCache {
    pub(crate) fn open() -> Arc<Self> {
        Arc::new(Self {
            dir: PathBuf::from("data/span-index"),
            memory: Mutex::new(Memory {
                order: VecDeque::new(),
                files: HashMap::new(),
            }),
        })
    }

    pub(crate) fn cached(&self, sha256: &str) -> Option<Arc<SpanFileIndex>> {
        if let Some(hit) = self.memory_hit(sha256) {
            return Some(hit);
        }
        let index = self.read_disk(sha256).ok()?;
        let index = Arc::new(index);
        self.remember(sha256, Arc::clone(&index));
        Some(index)
    }

    pub(crate) fn get_or_build(
        &self,
        sha256: &str,
        zip_bytes: &[u8],
    ) -> Result<Arc<SpanFileIndex>, String> {
        if let Some(hit) = self.cached(sha256) {
            return Ok(hit);
        }
        let index = Arc::new(span_index_from_zip(zip_bytes)?);
        if let Err(error) = self.write_disk(sha256, &index) {
            tracing::warn!(%error, "SPAN index was not stored");
        }
        self.remember(sha256, Arc::clone(&index));
        Ok(index)
    }

    fn memory_hit(&self, sha256: &str) -> Option<Arc<SpanFileIndex>> {
        self.lock().files.get(sha256).map(Arc::clone)
    }

    fn remember(&self, sha256: &str, index: Arc<SpanFileIndex>) {
        let mut memory = self.lock();
        if memory.files.contains_key(sha256) {
            return;
        }
        memory.order.push_back(sha256.to_owned());
        memory.files.insert(sha256.to_owned(), index);
        while memory.order.len() > MEMORY_FILES {
            if let Some(old) = memory.order.pop_front() {
                memory.files.remove(&old);
            }
        }
    }

    fn read_disk(&self, sha256: &str) -> Result<SpanFileIndex, String> {
        let path = self.path(sha256)?;
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        match read_index(&bytes, sha256) {
            Ok(index) => Ok(index),
            Err(error) => {
                let _ = fs::remove_file(&path);
                Err(error)
            }
        }
    }

    fn write_disk(&self, sha256: &str, index: &SpanFileIndex) -> Result<(), String> {
        let path = self.path(sha256)?;
        let parent = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let bytes = write_index(index, sha256)?;
        let temporary = path.with_extension("spanidx.tmp");
        {
            let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
            file.write_all(&bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
        }
        fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn path(&self, sha256: &str) -> Result<PathBuf, String> {
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("SPAN index name is not a sha256".to_owned());
        }
        Ok(self.dir.join(format!("{sha256}.spanidx")))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

pub(crate) fn write_index(index: &SpanFileIndex, sha256: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&decode_sha(sha256)?);
    write_date(&mut out, index.business_date);
    write_string(&mut out, &index.exchange)?;
    write_count(&mut out, index.families.len())?;
    for family in &index.families {
        write_string(&mut out, &family.cc)?;
        write_string(&mut out, &family.name)?;
        write_count(&mut out, family.portfolios.len())?;
        for (code, kind) in &family.portfolios {
            write_string(&mut out, code)?;
            write_string(&mut out, kind)?;
        }
    }
    write_count(&mut out, index.contracts.len())?;
    for contract in &index.contracts {
        write_kind(&mut out, contract.kind)?;
        write_string(&mut out, &contract.product)?;
        write_optional_date(&mut out, contract.expiry);
        write_optional_decimal(&mut out, contract.strike)?;
        write_decimal(&mut out, contract.settlement_price)?;
        write_optional_decimal(&mut out, contract.delta)?;
        write_decimal(&mut out, contract.cvf)?;
        write_decimal(&mut out, contract.scaling)?;
        write_count(&mut out, contract.values.len())?;
        for value in &contract.values {
            write_decimal(&mut out, *value)?;
        }
    }
    write_count(&mut out, index.commodities.len())?;
    for (code, commodity) in &index.commodities {
        write_string(&mut out, code)?;
        out.extend_from_slice(&commodity.risk_exponent.to_le_bytes());
        write_string(&mut out, &commodity.combination_method)?;
        out.push(u8::from(commodity.som_is_zero));
    }
    write_count(&mut out, index.underlyings.len())?;
    for (code, price) in &index.underlyings {
        write_string(&mut out, code)?;
        write_decimal(&mut out, *price)?;
    }
    write_count(&mut out, index.spreads.len())?;
    for (code, spreads) in &index.spreads {
        write_string(&mut out, code)?;
        write_count(&mut out, spreads.len())?;
        for spread in spreads {
            write_spread(&mut out, spread)?;
        }
    }
    write_count(&mut out, index.indexes.len())?;
    for code in &index.indexes {
        write_string(&mut out, code)?;
    }
    write_count(&mut out, index.spread_faults.len())?;
    for fault in &index.spread_faults {
        write_count(&mut out, fault.products.len())?;
        for product in &fault.products {
            write_string(&mut out, product)?;
        }
        write_string(&mut out, &fault.detail)?;
    }
    Ok(out)
}

pub(crate) fn read_index(bytes: &[u8], sha256: &str) -> Result<SpanFileIndex, String> {
    let mut reader = ByteReader { bytes, at: 0 };
    let magic = reader.take(8)?;
    if magic != MAGIC {
        return Err("SPAN index header is not recognized".to_owned());
    }
    if reader.byte()? != VERSION {
        return Err("SPAN index version is not supported".to_owned());
    }
    let stored = reader.take(32)?;
    if stored != decode_sha(sha256)? {
        return Err("SPAN index does not match the archived file".to_owned());
    }
    let business_date = reader.date()?;
    let exchange = reader.string()?;
    let family_count = reader.count()?;
    let mut families = Vec::with_capacity(family_count);
    for _ in 0..family_count {
        let cc = reader.string()?;
        let name = reader.string()?;
        let links = reader.count()?;
        let mut portfolios = Vec::with_capacity(links);
        for _ in 0..links {
            portfolios.push((reader.string()?, reader.string()?));
        }
        families.push(CommodityFamily {
            cc,
            name,
            portfolios,
        });
    }
    let contract_count = reader.count()?;
    let mut contracts = Vec::with_capacity(contract_count);
    for _ in 0..contract_count {
        let kind = reader.kind()?;
        let product = reader.string()?;
        let expiry = reader.optional_date()?;
        let strike = reader.optional_decimal()?;
        let settlement_price = reader.decimal()?;
        let delta = reader.optional_decimal()?;
        let cvf = reader.decimal()?;
        let scaling = reader.decimal()?;
        let values_len = reader.count()?;
        let mut values = Vec::with_capacity(values_len);
        for _ in 0..values_len {
            values.push(reader.decimal()?);
        }
        contracts.push(SpanContract {
            kind,
            product,
            expiry,
            strike,
            settlement_price,
            delta,
            cvf,
            scaling,
            values,
        });
    }
    let commodity_count = reader.count()?;
    let mut commodities = std::collections::BTreeMap::new();
    for _ in 0..commodity_count {
        let code = reader.string()?;
        let risk_exponent = i32::from_le_bytes(reader.take(4)?.try_into().expect("four bytes"));
        let combination_method = reader.string()?;
        let som_is_zero = match reader.byte()? {
            0 => false,
            1 => true,
            _ => return Err("SPAN index commodity flag is invalid".to_owned()),
        };
        if commodities
            .insert(
                code.clone(),
                Commodity {
                    risk_exponent,
                    combination_method,
                    som_is_zero,
                },
            )
            .is_some()
        {
            return Err(format!("SPAN index repeats combined commodity {code}"));
        }
    }
    let underlying_count = reader.count()?;
    let mut underlyings = std::collections::BTreeMap::new();
    for _ in 0..underlying_count {
        let code = reader.string()?;
        let price = reader.decimal()?;
        if underlyings.insert(code.clone(), price).is_some() {
            return Err(format!("SPAN index repeats underlying {code}"));
        }
    }
    let spread_groups = reader.count()?;
    let mut spreads = std::collections::BTreeMap::new();
    for _ in 0..spread_groups {
        let code = reader.string()?;
        let count = reader.count()?;
        let mut group = Vec::with_capacity(count);
        for _ in 0..count {
            group.push(reader.spread()?);
        }
        if spreads.insert(code.clone(), group).is_some() {
            return Err(format!("SPAN index repeats spreads for {code}"));
        }
    }
    let index_count = reader.count()?;
    let mut indexes = std::collections::BTreeSet::new();
    for _ in 0..index_count {
        indexes.insert(reader.string()?);
    }
    let fault_count = reader.count()?;
    let mut spread_faults = Vec::with_capacity(fault_count);
    for _ in 0..fault_count {
        let count = reader.count()?;
        let mut products = Vec::with_capacity(count);
        for _ in 0..count {
            products.push(reader.string()?);
        }
        spread_faults.push(SpreadFault {
            products,
            detail: reader.string()?,
        });
    }
    if reader.at != reader.bytes.len() {
        return Err("SPAN index has trailing bytes".to_owned());
    }
    Ok(SpanFileIndex {
        business_date,
        exchange,
        families,
        contracts,
        commodities,
        underlyings,
        spreads,
        indexes,
        spread_faults,
    })
}

fn write_spread(out: &mut Vec<u8>, spread: &CalendarSpread) -> Result<(), String> {
    out.extend_from_slice(&spread.number.to_le_bytes());
    write_decimal(out, spread.charge)?;
    write_string(out, &spread.method)?;
    write_date(out, spread.legs[0].0);
    write_decimal(out, spread.legs[0].1)?;
    write_date(out, spread.legs[1].0);
    write_decimal(out, spread.legs[1].1)?;
    Ok(())
}

fn write_kind(out: &mut Vec<u8>, kind: ContractKind) -> Result<(), String> {
    out.push(match kind {
        ContractKind::Future => 1,
        ContractKind::Call => 2,
        ContractKind::Put => 3,
        ContractKind::Stock => 4,
    });
    Ok(())
}

fn write_optional_date(out: &mut Vec<u8>, date: Option<NaiveDate>) {
    match date {
        Some(date) => {
            out.push(1);
            write_date(out, date);
        }
        None => out.push(0),
    }
}

fn write_date(out: &mut Vec<u8>, date: NaiveDate) {
    let stamp = u32::try_from(date.year()).unwrap_or(0) * 10_000 + date.month() * 100 + date.day();
    out.extend_from_slice(&stamp.to_le_bytes());
}

fn write_optional_decimal(out: &mut Vec<u8>, value: Option<Decimal>) -> Result<(), String> {
    match value {
        Some(value) => {
            out.push(1);
            write_decimal(out, value)
        }
        None => {
            out.push(0);
            Ok(())
        }
    }
}

fn write_decimal(out: &mut Vec<u8>, value: Decimal) -> Result<(), String> {
    let scale = value.scale();
    if scale > 28 {
        return Err("SPAN index decimal scale is too large".to_owned());
    }
    out.extend_from_slice(&value.mantissa().to_le_bytes());
    out.push(u8::try_from(scale).unwrap_or(255));
    Ok(())
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    let length =
        u32::try_from(value.len()).map_err(|_| "SPAN index string is too long".to_owned())?;
    if value.len() > MAX_STRING {
        return Err("SPAN index string is too long".to_owned());
    }
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_count(out: &mut Vec<u8>, count: usize) -> Result<(), String> {
    let count = u32::try_from(count).map_err(|_| "SPAN index is too large".to_owned())?;
    if count > MAX_COUNT {
        return Err("SPAN index is too large".to_owned());
    }
    out.extend_from_slice(&count.to_le_bytes());
    Ok(())
}

fn decode_sha(sha256: &str) -> Result<[u8; 32], String> {
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("SPAN index name is not a sha256".to_owned());
    }
    let mut out = [0; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&sha256[index * 2..index * 2 + 2], 16)
            .map_err(|_| "SPAN index name is not a sha256".to_owned())?;
    }
    Ok(out)
}

struct ByteReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> ByteReader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(length)
            .ok_or_else(|| "SPAN index is truncated".to_owned())?;
        if end > self.bytes.len() {
            return Err("SPAN index is truncated".to_owned());
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn count(&mut self) -> Result<usize, String> {
        let bytes = self.take(4)?;
        let count = u32::from_le_bytes(bytes.try_into().expect("four bytes"));
        if count > MAX_COUNT {
            return Err("SPAN index is too large".to_owned());
        }
        Ok(usize::try_from(count).unwrap_or(0))
    }

    fn string(&mut self) -> Result<String, String> {
        let length = self.count()?;
        if length > MAX_STRING {
            return Err("SPAN index string is too long".to_owned());
        }
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| "SPAN index string is not UTF-8".to_owned())
    }

    fn date(&mut self) -> Result<NaiveDate, String> {
        let stamp = u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes"));
        NaiveDate::from_ymd_opt(
            i32::try_from(stamp / 10_000).unwrap_or(0),
            stamp / 100 % 100,
            stamp % 100,
        )
        .ok_or_else(|| format!("SPAN index date {stamp} is not a calendar date"))
    }

    fn optional_date(&mut self) -> Result<Option<NaiveDate>, String> {
        match self.byte()? {
            0 => Ok(None),
            1 => Ok(Some(self.date()?)),
            _ => Err("SPAN index date flag is invalid".to_owned()),
        }
    }

    fn decimal(&mut self) -> Result<Decimal, String> {
        let mantissa = i128::from_le_bytes(self.take(16)?.try_into().expect("sixteen bytes"));
        let scale = self.byte()?;
        if scale > 28 {
            return Err("SPAN index decimal scale is too large".to_owned());
        }
        Decimal::try_from_i128_with_scale(mantissa, u32::from(scale))
            .map_err(|_| "SPAN index decimal is outside the supported range".to_owned())
    }

    fn optional_decimal(&mut self) -> Result<Option<Decimal>, String> {
        match self.byte()? {
            0 => Ok(None),
            1 => Ok(Some(self.decimal()?)),
            _ => Err("SPAN index decimal flag is invalid".to_owned()),
        }
    }

    fn kind(&mut self) -> Result<ContractKind, String> {
        match self.byte()? {
            1 => Ok(ContractKind::Future),
            2 => Ok(ContractKind::Call),
            3 => Ok(ContractKind::Put),
            4 => Ok(ContractKind::Stock),
            _ => Err("SPAN index contract kind is invalid".to_owned()),
        }
    }

    fn spread(&mut self) -> Result<CalendarSpread, String> {
        let number = u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes"));
        let charge = self.decimal()?;
        let method = self.string()?;
        let first = (self.date()?, self.decimal()?);
        let second = (self.date()?, self.decimal()?);
        Ok(CalendarSpread {
            number,
            charge,
            method,
            legs: [first, second],
        })
    }
}
