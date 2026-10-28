//! XML-based SPAN risk parameter file, format 4.00.
//!
//! NSE publishes this layout as a `.spn` member. PC-SPAN calls it the XML-based
//! SPAN file, distinct from the positional `.pa2` and `.par` layouts. Those
//! positional layouts are not parsed. The tag names come from NSCCL circular
//! 1425/2012. NSE Clearing states that a risk-array loss is positive and a gain
//! is negative. This parser checks that document. It does not calculate margin.

use chrono::{NaiveDate, NaiveDateTime};
use flate2::read::DeflateDecoder;
use quick_xml::Reader;
use quick_xml::events::Event;
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::io::Read;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpanXmlReport {
    pub scan_points: Vec<u32>,
    pub rate_count: u32,
    pub risk_array_count: u64,
}

/// The `<created>` clock in an NSE SPAN zip. The value is `YYYYMMDDHHMM` with no
/// offset. The observed files use India Standard Time.
pub(crate) fn span_created_timestamp(zip_prefix: &[u8]) -> Result<NaiveDateTime, String> {
    let payload = span_member_prefix(zip_prefix)?;
    let text = String::from_utf8_lossy(&payload);
    let start = text
        .find("<created>")
        .ok_or_else(|| "SPAN file has no created time".to_owned())?
        + "<created>".len();
    let rest = &text[start..];
    let end = rest
        .find("</created>")
        .ok_or_else(|| "SPAN created time is truncated".to_owned())?;
    let stamp = rest[..end].trim();
    NaiveDateTime::parse_from_str(stamp, "%Y%m%d%H%M")
        .map_err(|_| format!("SPAN created time {stamp} is not YYYYMMDDHHMM"))
}

fn span_member_prefix(zip_prefix: &[u8]) -> Result<Vec<u8>, String> {
    if zip_prefix.len() < 30 || &zip_prefix[..4] != b"PK\x03\x04" {
        return Err("SPAN zip prefix is not a zip local header".to_owned());
    }
    let method = u16::from_le_bytes([zip_prefix[8], zip_prefix[9]]);
    let name_len = u16::from_le_bytes([zip_prefix[26], zip_prefix[27]]) as usize;
    let extra_len = u16::from_le_bytes([zip_prefix[28], zip_prefix[29]]) as usize;
    let start = 30 + name_len + extra_len;
    if zip_prefix.len() <= start {
        return Err("SPAN zip prefix ends before the member".to_owned());
    }
    let compressed = &zip_prefix[start..];
    match method {
        0 => Ok(compressed.to_vec()),
        8 => inflate_until_created(compressed),
        _ => Err(format!(
            "SPAN zip compression method {method} is not deflate"
        )),
    }
}

fn inflate_until_created(compressed: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = DeflateDecoder::new(compressed);
    let mut out = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match decoder.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                out.extend_from_slice(&buffer[..read]);
                if out.windows(10).any(|window| window == b"</created>") || out.len() > 65_536 {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof && !out.is_empty() => {
                break;
            }
            Err(error) => {
                return Err(format!("SPAN zip member could not be inflated: {error}"));
            }
        }
    }
    if out.is_empty() {
        return Err("SPAN zip member produced no XML".to_owned());
    }
    Ok(out)
}

pub(crate) fn parse_span_xml(
    bytes: &[u8],
    trading_date: NaiveDate,
) -> Result<SpanXmlReport, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack = Vec::new();
    let mut state = ParseState::default();

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("SPAN XML could not be read: {error}"))?;
        match event {
            Event::Start(element) => {
                if stack.len() >= 64 {
                    return Err("SPAN XML nesting exceeds the supported depth".to_owned());
                }
                let tag = classify(element.local_name().as_ref());
                open_tag(&mut state, tag)?;
                stack.push(tag);
            }
            Event::Empty(element) => {
                let tag = classify(element.local_name().as_ref());
                open_tag(&mut state, tag)?;
                close_tag(&mut state, tag, stack.last().copied())?;
            }
            Event::Text(text) => {
                let value = text
                    .unescape()
                    .map_err(|error| format!("SPAN XML text could not be decoded: {error}"))?;
                if !value.is_empty() {
                    take_text(&mut state, &stack, &value)?;
                }
            }
            Event::CData(text) => {
                let value = text
                    .decode()
                    .map_err(|error| format!("SPAN XML text could not be decoded: {error}"))?;
                if !value.is_empty() {
                    take_text(&mut state, &stack, &value)?;
                }
            }
            Event::End(element) => {
                let tag = classify(element.local_name().as_ref());
                let opened = stack
                    .pop()
                    .ok_or_else(|| "SPAN XML has an extra end tag".to_owned())?;
                if opened != tag {
                    return Err("SPAN XML end tag does not match the open element".to_owned());
                }
                close_tag(&mut state, tag, stack.last().copied())?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err("SPAN XML ended before all elements were closed".to_owned());
    }
    state.finish(trading_date)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Other,
    SpanFile,
    FileFormat,
    PointInTime,
    Date,
    ClearingOrg,
    Ec,
    Exchange,
    Exch,
    PointDef,
    Rate,
    ScanPointDef,
    Point,
    Ra,
    Value,
}

fn classify(name: &[u8]) -> Tag {
    match name {
        b"spanFile" => Tag::SpanFile,
        b"fileFormat" => Tag::FileFormat,
        b"pointInTime" => Tag::PointInTime,
        b"date" => Tag::Date,
        b"clearingOrg" => Tag::ClearingOrg,
        b"ec" => Tag::Ec,
        b"exchange" => Tag::Exchange,
        b"exch" => Tag::Exch,
        b"pointDef" => Tag::PointDef,
        b"r" => Tag::Rate,
        b"scanPointDef" => Tag::ScanPointDef,
        b"point" => Tag::Point,
        b"ra" => Tag::Ra,
        b"a" => Tag::Value,
        _ => Tag::Other,
    }
}

#[derive(Default)]
struct ParseState {
    seen_root: bool,
    file_format: Option<String>,
    business_date: Option<NaiveDate>,
    clearing_org: Option<String>,
    saw_nse: bool,
    completed: BTreeMap<String, Vec<u32>>,
    open_rate: Option<String>,
    open_points: Vec<u32>,
    open_array_rate: Option<String>,
    open_values: u16,
    value_has_text: bool,
    risk_array_count: u64,
}

fn open_tag(state: &mut ParseState, tag: Tag) -> Result<(), String> {
    match tag {
        Tag::SpanFile => {
            if state.seen_root {
                return Err("SPAN document has more than one spanFile element".to_owned());
            }
            state.seen_root = true;
        }
        Tag::PointDef => {
            state.open_rate = None;
            state.open_points.clear();
        }
        Tag::Ra => {
            state.open_array_rate = None;
            state.open_values = 0;
            state.value_has_text = false;
        }
        Tag::Value => state.value_has_text = false,
        _ => {}
    }
    Ok(())
}

fn close_tag(state: &mut ParseState, tag: Tag, parent: Option<Tag>) -> Result<(), String> {
    match tag {
        Tag::Value if parent == Some(Tag::Ra) && !state.value_has_text => {
            return Err("SPAN risk array value is empty".to_owned());
        }
        Tag::PointDef => {
            let rate = state
                .open_rate
                .take()
                .ok_or_else(|| "SPAN pointDef is missing its rate id".to_owned())?;
            if state.open_points.is_empty() {
                return Err(format!("SPAN pointDef rate {rate} has no scan points"));
            }
            let points = std::mem::take(&mut state.open_points);
            validate_point_series(&rate, &points)?;
            if state.completed.insert(rate.clone(), points).is_some() {
                return Err(format!("SPAN pointDef rate {rate} is repeated"));
            }
        }
        Tag::Ra => {
            let rate = state
                .open_array_rate
                .take()
                .ok_or_else(|| "SPAN risk array is missing its rate id".to_owned())?;
            let points = state.completed.get(&rate).ok_or_else(|| {
                format!("SPAN risk array rate {rate} has no completed scan-point definition")
            })?;
            let found = usize::from(state.open_values);
            if found != points.len() {
                return Err(format!(
                    "SPAN risk array rate {rate} has {found} values for {} scan points",
                    points.len()
                ));
            }
            state.open_values = 0;
            state.value_has_text = false;
            state.risk_array_count = state.risk_array_count.saturating_add(1);
        }
        _ => {}
    }
    Ok(())
}

fn take_text(state: &mut ParseState, stack: &[Tag], value: &str) -> Result<(), String> {
    let parent = stack.last().copied().unwrap_or(Tag::Other);
    let grandparent = stack.get(stack.len().wrapping_sub(2)).copied();
    match parent {
        Tag::FileFormat => {
            if state.file_format.replace(value.to_owned()).is_some() {
                return Err("SPAN fileFormat is repeated".to_owned());
            }
        }
        Tag::Date if grandparent == Some(Tag::PointInTime) => {
            let parsed = NaiveDate::parse_from_str(value, "%Y%m%d")
                .map_err(|_| format!("SPAN business date `{value}` is not YYYYMMDD"))?;
            if state.business_date.replace(parsed).is_some() {
                return Err("SPAN business date is repeated".to_owned());
            }
        }
        Tag::Ec if grandparent == Some(Tag::ClearingOrg) => {
            if state.clearing_org.replace(value.to_owned()).is_some() {
                return Err("SPAN clearing organization is repeated".to_owned());
            }
        }
        Tag::Exch if grandparent == Some(Tag::Exchange) && value == "NSE" => state.saw_nse = true,
        Tag::Rate if grandparent == Some(Tag::PointDef) => {
            if state.open_rate.is_some() {
                return Err("SPAN pointDef rate id is repeated".to_owned());
            }
            state.open_rate = Some(value.to_owned());
        }
        Tag::Point if parent_is_scan(stack) => {
            let point = value
                .parse::<u32>()
                .map_err(|_| format!("SPAN scan point `{value}` is not an integer"))?;
            if !(1..=16).contains(&point) {
                return Err(format!(
                    "SPAN scan point {point} is outside the documented range 1 to 16"
                ));
            }
            state.open_points.push(point);
        }
        Tag::Rate if grandparent == Some(Tag::Ra) => {
            if state.open_array_rate.is_some() {
                return Err("SPAN risk array rate id is repeated".to_owned());
            }
            state.open_array_rate = Some(value.to_owned());
        }
        Tag::Value if grandparent == Some(Tag::Ra) => {
            Decimal::from_str(value)
                .map_err(|_| format!("SPAN risk array value `{value}` is not a decimal number"))?;
            state.value_has_text = true;
            state.open_values = state.open_values.checked_add(1).ok_or_else(|| {
                "SPAN risk array has more values than the documented scan points".to_owned()
            })?;
            if state.open_values > 16 {
                return Err(
                    "SPAN risk array has more than 16 values, above the documented scan points"
                        .to_owned(),
                );
            }
        }
        _ => {}
    }
    Ok(())
}

fn parent_is_scan(stack: &[Tag]) -> bool {
    stack.len() >= 2 && stack[stack.len() - 2] == Tag::ScanPointDef
}

fn validate_point_series(rate: &str, points: &[u32]) -> Result<(), String> {
    let count = u32::try_from(points.len()).unwrap_or(u32::MAX);
    let expected = (1..=count).collect::<Vec<_>>();
    if points != expected {
        return Err(format!(
            "SPAN scan points for rate {rate} are not the contiguous series 1 to {}",
            points.len()
        ));
    }
    Ok(())
}

impl ParseState {
    fn finish(self, trading_date: NaiveDate) -> Result<SpanXmlReport, String> {
        if !self.seen_root {
            return Err(
                "member is not an XML-based SPAN risk parameter file: spanFile is missing"
                    .to_owned(),
            );
        }
        match self.file_format.as_deref() {
            Some("4.00") => {}
            Some(other) => {
                return Err(format!("SPAN fileFormat `{other}` is not 4.00"));
            }
            None => return Err("SPAN fileFormat is missing".to_owned()),
        }
        let Some(business_date) = self.business_date else {
            return Err("SPAN business date is missing".to_owned());
        };
        if business_date != trading_date {
            return Err(format!(
                "SPAN business date {business_date} does not match the requested trading date {trading_date}"
            ));
        }
        match self.clearing_org.as_deref() {
            Some("NSCCL") => {}
            Some(other) => {
                return Err(format!("SPAN clearing organization `{other}` is not NSCCL"));
            }
            None => return Err("SPAN clearing organization is missing".to_owned()),
        }
        if !self.saw_nse {
            return Err("SPAN file does not name exchange NSE".to_owned());
        }
        if self.completed.is_empty() {
            return Err("SPAN file has no scan-point definition".to_owned());
        }
        if self.risk_array_count == 0 {
            return Err("SPAN file has no risk array".to_owned());
        }
        let rate_count = u32::try_from(self.completed.len()).unwrap_or(u32::MAX);
        let scan_points = if self.completed.len() == 1 {
            self.completed.into_values().next().unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(SpanXmlReport {
            scan_points,
            rate_count,
            risk_array_count: self.risk_array_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_span_xml;
    use chrono::{NaiveDate, NaiveDateTime};

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 27).expect("date")
    }

    fn document() -> String {
        r#"<?xml version="1.0"?>
<spanFile>
<fileFormat>4.00</fileFormat>
<pointInTime><date>20261027</date></pointInTime>
<clearingOrg><ec>NSCCL</ec></clearingOrg>
<pointDef><r>1</r>
<scanPointDef><point>1</point></scanPointDef>
<scanPointDef><point>2</point></scanPointDef>
</pointDef>
<exchange><exch>NSE</exch>
<fut><ra><r>1</r><a>1.50</a><a>-0.25</a></ra></fut>
</exchange>
</spanFile>"#
            .to_owned()
    }

    #[test]
    fn span_xml_accepts_the_documented_risk_array_shape() {
        let report = parse_span_xml(document().as_bytes(), date()).expect("span");
        assert_eq!(report.scan_points, vec![1, 2]);
        assert_eq!(report.rate_count, 1);
        assert_eq!(report.risk_array_count, 1);
    }

    #[test]
    fn span_xml_rejects_a_positional_file_a_wrong_date_and_a_short_array() {
        let trading_date = date();
        assert!(parse_span_xml(b"SPAN HEADER 0001", trading_date).is_err());
        let wrong_day = document().replace("20261027", "20261028");
        assert!(parse_span_xml(wrong_day.as_bytes(), trading_date).is_err());
        let short = document().replace("<a>-0.25</a>", "");
        assert!(parse_span_xml(short.as_bytes(), trading_date).is_err());
        let other_format = document().replace(
            "<fileFormat>4.00</fileFormat>",
            "<fileFormat>3.00</fileFormat>",
        );
        assert!(parse_span_xml(other_format.as_bytes(), trading_date).is_err());
    }

    #[test]
    fn the_created_clock_is_read_from_a_deflated_span_member() {
        let payload = b"<spanFile><created>202610281403</created></spanFile>";
        let compressed = {
            use flate2::Compression;
            use flate2::write::DeflateEncoder;
            use std::io::Write;
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(payload).expect("deflate");
            encoder.finish().expect("finish")
        };
        let zip = local_zip(
            b"nsccl.20261028.i04.spn",
            8,
            &compressed,
            payload.len() as u32,
        );
        let created = super::span_created_timestamp(&zip).expect("created");
        assert_eq!(
            created,
            NaiveDateTime::parse_from_str("202610281403", "%Y%m%d%H%M").expect("stamp")
        );
        assert!(super::span_created_timestamp(b"not a zip").is_err());
    }

    fn local_zip(name: &[u8], method: u16, payload: &[u8], uncompressed: u32) -> Vec<u8> {
        let mut zip = Vec::new();
        zip.extend_from_slice(b"PK\x03\x04");
        zip.extend_from_slice(&20_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&method.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u32.to_le_bytes());
        zip.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        zip.extend_from_slice(&uncompressed.to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(name);
        zip.extend_from_slice(payload);
        zip
    }
}
