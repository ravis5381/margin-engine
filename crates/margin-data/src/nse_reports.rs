//! Verified NSE equity-derivatives report names and container checks.
//!
//! The report names, filenames, and ZIP member names below were observed on
//! downloads from `https://www.nseindia.com/api/reports` for 27 October 2026,
//! and the begin-of-day member name was observed again for 28 October 2026.
//! The SPAN member is the XML-based SPAN 4.00 document. Positional SPAN layouts
//! are not parsed. Risk-array values are checked as decimals and are not copied
//! onto contracts.

use crate::DataError;
use crate::span_xml::{SpanXmlReport, parse_span_xml};
use crate::zip_safe::{ZipLimits, extract_zip_to_memory};
use chrono::{Datelike, NaiveDate};
use flate2::read::GzDecoder;
use margin_domain::SnapshotLocator;
use reqwest::Url;
use serde::Serialize;
use std::io::Read;

pub const NSE_SOURCE_ID: &str = "nse-clearing-equity-derivatives";
pub const NSE_HOST: &str = "www.nseindia.com";
pub const NSE_VAR_HOST: &str = "nsearchives.nseindia.com";
pub const NSE_REPORTS_PATH: &str = "/api/reports";
pub const NSE_SESSION_URL: &str = "https://www.nseindia.com/all-reports-derivatives";
pub const NSE_SESSION_REFERER: &str = "https://www.nseindia.com/";
pub const NSE_REFERER: &str = "https://www.nseindia.com/all-reports-derivatives";
pub const NSE_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
pub const SPAN_FILE_FORMAT: &str = "nse-span-xml-4.00-zip";
pub const PARSER_VERSION: &str = "span-xml-4.00";

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NseReportKind {
    SpanBeginOfDay,
    SpanIntraday(u8),
    SpanEndOfDay,
    ExposureLimit,
    ContractElm,
}

impl NseReportKind {
    pub fn parse(value: &str) -> Result<Self, DataError> {
        let kind = match value {
            "span-begin-of-day" => Self::SpanBeginOfDay,
            "span-intraday-1" => Self::SpanIntraday(1),
            "span-intraday-2" => Self::SpanIntraday(2),
            "span-intraday-3" => Self::SpanIntraday(3),
            "span-intraday-4" => Self::SpanIntraday(4),
            "span-end-of-day" => Self::SpanEndOfDay,
            "exposure-limit" => Self::ExposureLimit,
            "contract-elm" => Self::ContractElm,
            other => {
                return Err(DataError::Configuration {
                    detail: format!(
                        "report kind `{other}` must be one of {}",
                        Self::codes().join(", ")
                    ),
                });
            }
        };
        kind.ensure_valid()?;
        Ok(kind)
    }

    pub fn margin_files() -> [Self; 8] {
        [
            Self::SpanEndOfDay,
            Self::SpanIntraday(4),
            Self::SpanIntraday(3),
            Self::SpanIntraday(2),
            Self::SpanIntraday(1),
            Self::SpanBeginOfDay,
            Self::ExposureLimit,
            Self::ContractElm,
        ]
    }

    pub fn is_span(self) -> bool {
        matches!(
            self,
            Self::SpanBeginOfDay | Self::SpanIntraday(_) | Self::SpanEndOfDay
        )
    }

    pub const fn codes() -> &'static [&'static str] {
        &[
            "span-begin-of-day",
            "span-intraday-1",
            "span-intraday-2",
            "span-intraday-3",
            "span-intraday-4",
            "span-end-of-day",
            "exposure-limit",
            "contract-elm",
        ]
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::SpanBeginOfDay => "span-begin-of-day",
            Self::SpanIntraday(1) => "span-intraday-1",
            Self::SpanIntraday(2) => "span-intraday-2",
            Self::SpanIntraday(3) => "span-intraday-3",
            Self::SpanIntraday(4) => "span-intraday-4",
            Self::SpanIntraday(_) => "span-intraday",
            Self::SpanEndOfDay => "span-end-of-day",
            Self::ExposureLimit => "exposure-limit",
            Self::ContractElm => "contract-elm",
        }
    }

    pub fn ensure_valid(self) -> Result<(), DataError> {
        if let Self::SpanIntraday(sequence) = self
            && !(1..=4).contains(&sequence)
        {
            return Err(DataError::Configuration {
                detail: "NSE intraday SPAN reports are numbered 1 to 4".to_owned(),
            });
        }
        Ok(())
    }

    pub fn report_name(self) -> &'static str {
        match self {
            Self::SpanBeginOfDay => "F&O - Span Risk Parameter File (Begin of day)",
            Self::SpanIntraday(1) => "F&O - Span Risk Parameter File (1st intra-day)",
            Self::SpanIntraday(2) => "F&O - Span Risk Parameter File (2nd intra-day)",
            Self::SpanIntraday(3) => "F&O - Span Risk Parameter File (3rd intra-day)",
            Self::SpanIntraday(4) => "F&O - Span Risk Parameter File (4th intra-day)",
            Self::SpanIntraday(_) => "F&O - Span Risk Parameter File",
            Self::SpanEndOfDay => "F&O - Span Risk Parameter File (End of day)",
            Self::ExposureLimit => "F&O - Exposure Limit file",
            Self::ContractElm => "F&O-Contract wise ELM file",
        }
    }

    pub fn snapshot(self) -> Option<SnapshotLocator> {
        match self {
            Self::SpanBeginOfDay => Some(SnapshotLocator::begin_of_day()),
            Self::SpanEndOfDay => Some(SnapshotLocator::end_of_day()),
            Self::SpanIntraday(sequence) if (1..=4).contains(&sequence) => {
                Some(SnapshotLocator::intraday(u32::from(sequence)))
            }
            Self::SpanIntraday(_) | Self::ExposureLimit | Self::ContractElm => None,
        }
    }

    pub fn auxiliary_kind(self) -> Option<&'static str> {
        match self {
            Self::ExposureLimit => Some("exposure_limit_csv"),
            Self::ContractElm => Some("contract_elm_gzip"),
            _ => None,
        }
    }

    pub fn expected_filename(self, trading_date: NaiveDate) -> Result<String, DataError> {
        self.ensure_valid()?;
        let compact = compact_date(trading_date);
        let day_first = day_first(trading_date);
        Ok(match self {
            Self::SpanBeginOfDay => format!("nsccl.{compact}.i1.zip"),
            Self::SpanIntraday(sequence) => format!("nsccl.{compact}.i{}.zip", sequence + 1),
            Self::SpanEndOfDay => format!("nsccl.{compact}.s.zip"),
            Self::ExposureLimit => format!("ael_{day_first}.csv"),
            Self::ContractElm => format!("F_AEL_OPT_CONTRACTS_{day_first}.csv.gz"),
        })
    }

    pub fn expected_span_member(
        self,
        trading_date: NaiveDate,
    ) -> Result<Option<String>, DataError> {
        self.ensure_valid()?;
        let compact = compact_date(trading_date);
        Ok(match self {
            Self::SpanBeginOfDay => Some(format!("nsccl.{compact}.i01.spn")),
            Self::SpanIntraday(sequence) => Some(format!("nsccl.{compact}.i0{}.spn", sequence + 1)),
            Self::SpanEndOfDay => Some(format!("nsccl.{compact}.s.spn")),
            Self::ExposureLimit | Self::ContractElm => None,
        })
    }

    pub fn expected_media_type(self) -> &'static [&'static str] {
        match self {
            Self::SpanBeginOfDay | Self::SpanIntraday(_) | Self::SpanEndOfDay => {
                &["application/zip"]
            }
            Self::ExposureLimit => &["text/csv"],
            Self::ContractElm => &["application/x-gzip", "application/gzip"],
        }
    }
}

pub fn reports_url(kind: NseReportKind, trading_date: NaiveDate) -> Result<Url, DataError> {
    kind.ensure_valid()?;
    let mut url = Url::parse(&format!("https://{NSE_HOST}{NSE_REPORTS_PATH}")).map_err(|_| {
        DataError::Configuration {
            detail: "NSE reports URL could not be built".to_owned(),
        }
    })?;
    let archives = serde_json::to_string(&vec![ArchiveCard {
        name: kind.report_name(),
        type_: "archives",
        category: "derivatives",
        section: "equity",
    }])
    .map_err(|_| DataError::Configuration {
        detail: "NSE report query could not be encoded".to_owned(),
    })?;
    url.query_pairs_mut()
        .append_pair("archives", &archives)
        .append_pair("date", &display_date(trading_date))
        .append_pair("type", "equity")
        .append_pair("mode", "single");
    Ok(url)
}

pub fn contract_master_url(trading_date: NaiveDate) -> Result<Url, DataError> {
    let filename = format!(
        "NSE_FO_contract_{:02}{:02}{:04}.csv.gz",
        trading_date.day(),
        trading_date.month(),
        trading_date.year()
    );
    Url::parse(&format!("https://{NSE_VAR_HOST}/content/fo/{filename}")).map_err(|_| {
        DataError::Configuration {
            detail: "contract master URL could not be built".to_owned(),
        }
    })
}

pub fn cash_var_url(trading_date: NaiveDate, batch: u8) -> Result<Url, DataError> {
    if !(1..=6).contains(&batch) {
        return Err(DataError::Configuration {
            detail: "cash-market VaR batches are numbered 1 to 6".to_owned(),
        });
    }
    let filename = format!(
        "C_VAR1_{:02}{:02}{:04}_{batch}.DAT",
        trading_date.day(),
        trading_date.month(),
        trading_date.year()
    );
    Url::parse(&format!(
        "https://{NSE_VAR_HOST}/archives/nsccl/var/{filename}"
    ))
    .map_err(|_| DataError::Configuration {
        detail: "cash-market VaR URL could not be built".to_owned(),
    })
}

pub fn is_verified_reports_endpoint(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(NSE_HOST)
        && (url.path() == NSE_REPORTS_PATH || url.path() == "/api/reports/")
        && url.query().is_none()
        && url.fragment().is_none()
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

pub fn display_date(trading_date: NaiveDate) -> String {
    format!(
        "{:02}-{}-{}",
        trading_date.day(),
        MONTHS[trading_date.month0() as usize],
        trading_date.year()
    )
}

pub fn media_type_matches(header: &str, allowed: &[&str]) -> bool {
    let media = header
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    allowed.iter().any(|expected| media == *expected)
}

pub fn content_disposition_filename(header: &str) -> Result<String, DataError> {
    for part in header.split(';') {
        let part = part.trim();
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("filename") {
            continue;
        }
        let filename = value.trim().trim_matches('"').trim();
        if filename.is_empty()
            || filename.len() > 255
            || filename.contains('/')
            || filename.contains('\\')
            || filename.contains("..")
        {
            return Err(DataError::DownloadRejected {
                detail: "content disposition filename is empty or contains a path".to_owned(),
            });
        }
        return Ok(filename.to_owned());
    }
    Err(DataError::DownloadRejected {
        detail: "response did not name the downloaded file".to_owned(),
    })
}

pub(crate) fn check_span_container(
    bytes: &[u8],
    expected_member: &str,
    max_uncompressed_bytes: u64,
    trading_date: NaiveDate,
) -> Result<SpanXmlReport, String> {
    let entries = extract_zip_to_memory(bytes, ZipLimits::conservative(max_uncompressed_bytes))
        .map_err(|error| error.to_string())?;
    if entries.len() != 1 || entries[0].name != expected_member {
        let found = if entries.is_empty() {
            "no members".to_owned()
        } else {
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(format!(
            "SPAN zip member `{found}` did not match the verified name `{expected_member}`"
        ));
    }
    parse_span_xml(&entries[0].bytes, trading_date)
}

pub fn check_exposure_csv(bytes: &[u8], trading_date: NaiveDate) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "exposure file is not UTF-8".to_owned())?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    let expected = format!(
        "Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date {},Total applicable ELM%",
        display_date(trading_date)
    );
    if header != expected {
        return Err("exposure file header does not match the verified NSE column line".to_owned());
    }
    match lines.next() {
        Some(row) if !row.trim().is_empty() => Ok(()),
        _ => Err("exposure file has no data row".to_owned()),
    }
}

pub fn check_contract_elm_gzip(bytes: &[u8], max_uncompressed_bytes: u64) -> Result<(), String> {
    if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        return Err("contract ELM file is not a gzip payload".to_owned());
    }
    let mut decoder = GzDecoder::new(bytes);
    let mut stored = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|error| format!("contract ELM gzip could not be read: {error}"))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > max_uncompressed_bytes {
            return Err("contract ELM uncompressed size exceeds the configured limit".to_owned());
        }
        if stored.len() < 1024 {
            let room = 1024 - stored.len();
            stored.extend_from_slice(&buffer[..read.min(room)]);
        }
    }
    let text = String::from_utf8_lossy(&stored);
    let line = text.lines().next().unwrap_or_default();
    if line.is_empty() || !line.contains(',') || line.contains('\0') {
        return Err("contract ELM gzip does not begin with a comma-separated record".to_owned());
    }
    Ok(())
}

fn compact_date(trading_date: NaiveDate) -> String {
    format!(
        "{:04}{:02}{:02}",
        trading_date.year(),
        trading_date.month(),
        trading_date.day()
    )
}

fn day_first(trading_date: NaiveDate) -> String {
    format!(
        "{:02}{:02}{:04}",
        trading_date.day(),
        trading_date.month(),
        trading_date.year()
    )
}

#[derive(Serialize)]
struct ArchiveCard<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    type_: &'static str,
    category: &'static str,
    section: &'static str,
}

#[cfg(test)]
mod tests {
    use super::{
        NseReportKind, check_contract_elm_gzip, check_exposure_csv, check_span_container,
        content_disposition_filename, display_date, media_type_matches, reports_url,
    };
    use chrono::NaiveDate;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::{Cursor, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 27).expect("date")
    }

    #[test]
    fn reports_url_uses_the_verified_query() {
        let url = reports_url(NseReportKind::SpanEndOfDay, date()).expect("url");
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("www.nseindia.com"));
        assert_eq!(url.path(), "/api/reports");
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs,
            vec![
                (
                    "archives".to_owned(),
                    "[{\"name\":\"F&O - Span Risk Parameter File (End of day)\",\"type\":\"archives\",\"category\":\"derivatives\",\"section\":\"equity\"}]".to_owned()
                ),
                ("date".to_owned(), "27-Oct-2026".to_owned()),
                ("type".to_owned(), "equity".to_owned()),
                ("mode".to_owned(), "single".to_owned()),
            ]
        );
        assert_eq!(display_date(date()), "27-Oct-2026");
    }

    #[test]
    fn filenames_match_the_observed_content_disposition_names() {
        let date = date();
        let cases = [
            (
                NseReportKind::SpanBeginOfDay,
                "nsccl.20261027.i1.zip",
                Some("nsccl.20261027.i01.spn"),
            ),
            (
                NseReportKind::SpanIntraday(1),
                "nsccl.20261027.i2.zip",
                Some("nsccl.20261027.i02.spn"),
            ),
            (
                NseReportKind::SpanIntraday(2),
                "nsccl.20261027.i3.zip",
                Some("nsccl.20261027.i03.spn"),
            ),
            (
                NseReportKind::SpanIntraday(3),
                "nsccl.20261027.i4.zip",
                Some("nsccl.20261027.i04.spn"),
            ),
            (
                NseReportKind::SpanIntraday(4),
                "nsccl.20261027.i5.zip",
                Some("nsccl.20261027.i05.spn"),
            ),
            (
                NseReportKind::SpanEndOfDay,
                "nsccl.20261027.s.zip",
                Some("nsccl.20261027.s.spn"),
            ),
            (NseReportKind::ExposureLimit, "ael_27102026.csv", None),
            (
                NseReportKind::ContractElm,
                "F_AEL_OPT_CONTRACTS_27102026.csv.gz",
                None,
            ),
        ];
        for (kind, filename, member) in cases {
            assert_eq!(kind.expected_filename(date).expect("name"), filename);
            assert_eq!(
                kind.expected_span_member(date).expect("member").as_deref(),
                member
            );
        }
        let next = NaiveDate::from_ymd_opt(2026, 10, 28).expect("date");
        assert_eq!(
            NseReportKind::SpanBeginOfDay
                .expected_span_member(next)
                .expect("member")
                .as_deref(),
            Some("nsccl.20261028.i01.spn")
        );
    }

    #[test]
    fn span_container_accepts_only_the_expected_member_and_span_xml() {
        let member = "nsccl.20261027.s.spn";
        let xml = br#"<?xml version="1.0"?><spanFile><fileFormat>4.00</fileFormat><pointInTime><date>20261027</date></pointInTime><clearingOrg><ec>NSCCL</ec></clearingOrg><pointDef><r>1</r><scanPointDef><point>1</point></scanPointDef></pointDef><exchange><exch>NSE</exch><fut><ra><r>1</r><a>1.50</a></ra></fut></exchange></spanFile>"#;
        let bytes = zip_named(member, xml);
        let report = check_span_container(&bytes, member, 4096, date()).expect("span");
        assert_eq!(report.scan_points, vec![1]);
        assert_eq!(report.risk_array_count, 1);
        assert!(check_span_container(&bytes, "nsccl.20261027.i01.spn", 4096, date()).is_err());
        let unmarked = zip_named(member, b"not-span");
        assert!(check_span_container(&unmarked, member, 4096, date()).is_err());
    }

    #[test]
    fn exposure_and_elm_checks_reject_a_different_layout() {
        let csv = format!(
            "Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date {},Total applicable ELM%\n1,SYNTH,OTH,0,0,0\n",
            display_date(date())
        );
        assert!(check_exposure_csv(csv.as_bytes(), date()).is_ok());
        assert!(check_exposure_csv(b"Symbol,Percent\n", date()).is_err());

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(b"OPTSTK,SYNTH,29-OCT-2026,0,PE,0,0\n")
            .expect("gzip");
        let gzip = encoder.finish().expect("finish");
        assert!(check_contract_elm_gzip(&gzip, 1024).is_ok());
        assert!(check_contract_elm_gzip(b"not-gzip", 1024).is_err());
        assert!(media_type_matches("text/csv; charset=utf-8", &["text/csv"]));
        assert!(!media_type_matches("text/html", &["text/csv"]));
        let filename =
            content_disposition_filename("attachment; filename=\"nsccl.20261027.s.zip\"")
                .expect("filename");
        assert_eq!(filename, "nsccl.20261027.s.zip");
    }

    fn zip_named(name: &str, contents: &[u8]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            writer
                .start_file(name, SimpleFileOptions::default())
                .expect("start");
            writer.write_all(contents).expect("write");
            writer.finish().expect("finish");
        }
        cursor.into_inner()
    }
}
