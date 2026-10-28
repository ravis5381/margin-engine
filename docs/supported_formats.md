# Supported formats

NSE equity-derivatives files can be downloaded from the verified reports
endpoint and archived as the original bytes. The margin quote reads the SPAN
risk arrays, stock membership from the exposure file, and percents from the
contract-wise ELM file.

The verified endpoint is `https://www.nseindia.com/api/reports`. A request uses
the report name shown on the NSE equity-derivatives archives page, the date in
`DD-Mmm-YYYY` form, `type=equity`, and `mode=single`. The client first requests
`https://www.nseindia.com/all-reports-derivatives`. That page sets the session
cookie. The site root returns HTTP 403 and does not. A missing file for the
requested date is an error. Another
date is not substituted.

These names, filenames, and ZIP members were observed for 27 October 2026.
Begin-of-day for 28 October 2026 used the same member pattern.

| Report | `--kind` | Observed filename | Observed ZIP member | Archive |
| --- | --- | --- | --- | --- |
| F&O - Span Risk Parameter File (Begin of day) | `span-begin-of-day` | `nsccl.YYYYMMDD.i1.zip` | `nsccl.YYYYMMDD.i01.spn` | Draft risk dataset, snapshot `begin_of_day` |
| F&O - Span Risk Parameter File (1st intra-day) | `span-intraday-1` | `nsccl.YYYYMMDD.i2.zip` | `nsccl.YYYYMMDD.i02.spn` | Draft risk dataset, intraday sequence 1 |
| F&O - Span Risk Parameter File (2nd intra-day) | `span-intraday-2` | `nsccl.YYYYMMDD.i3.zip` | `nsccl.YYYYMMDD.i03.spn` | Draft risk dataset, intraday sequence 2 |
| F&O - Span Risk Parameter File (3rd intra-day) | `span-intraday-3` | `nsccl.YYYYMMDD.i4.zip` | `nsccl.YYYYMMDD.i04.spn` | Draft risk dataset, intraday sequence 3 |
| F&O - Span Risk Parameter File (4th intra-day) | `span-intraday-4` | `nsccl.YYYYMMDD.i5.zip` | `nsccl.YYYYMMDD.i05.spn` | Draft risk dataset, intraday sequence 4 |
| F&O - Span Risk Parameter File (End of day) | `span-end-of-day` | `nsccl.YYYYMMDD.s.zip` | `nsccl.YYYYMMDD.s.spn` | Draft risk dataset, snapshot `end_of_day` |
| F&O - Exposure Limit file | `exposure-limit` | `ael_DDMMYYYY.csv` | — | Auxiliary file. Header observed: `Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date <DD-Mmm-YYYY>,Total applicable ELM%` |
| F&O-Contract wise ELM file | `contract-elm` | `F_AEL_OPT_CONTRACTS_DDMMYYYY.csv.gz` | — | Auxiliary gzip. Seven columns, no header: instrument type (`FUTIDX`, `FUTSTK`, `OPTIDX`, `OPTSTK`), symbol, expiry `DD-MMM-YYYY`, strike (`-1` for a future), option type (`XX`, `CE`, `PE`), corporate action level, extreme loss percent |

A SPAN download is kept only when the response is HTTP 200, `application/zip`,
the filename above, and one ZIP member with that name. The member is the
XML-based SPAN risk parameter file (`spanFile`, `fileFormat` 4.00), the `.spn`
layout PC-SPAN reads. The positional `.pa2` and `.par` layouts are not parsed.
NSCCL circular 1425/2012 names the tags. The parser requires clearing
organization `NSCCL`, exchange `NSE`, a business date equal to the requested
trading date, scan points numbered `1` through `n` (at most 16), and one decimal
`a` value per scan point on every `ra`. NSE Clearing describes those values as
losses when positive and gains when negative. One rate stores those scan points.
Risk-array amounts stay in the archived file: the SPAN document has no lot size
and no index/stock class, so contract rows are not created. The dataset stays
`draft` and `pending`. It is not a published margin.

The exposure file is kept when the header line matches the observed columns,
including the requested trade date. The margin quote reads each symbol's total
applicable ELM percent from that file and uses it to tell a stock from an index.
The contract ELM file is kept when the gzip opens and begins with a
comma-separated record. The margin quote reads its extreme-loss percent.
`parser_status` on the archive row stays `not_implemented`.

A container mismatch is stored as a rejected row and the command fails. The
original bytes are still the archived body. A non-file response, including an
HTTP 404 page, is not archived.

ICCL is not supported. `https://www.icclindia.com/riskmgmt/RiskparameterNEW.aspx`
redirects to `https://www.bseclearing.com/riskmgmt/RiskparameterNEW.aspx`, which
is an HTML form. No file response was captured, and that host is not
allowlisted. `ingest --source ICCL` exits 3.

`www.nseclearing.in` remains an informational site. It is not the download
host for these files. An unofficial SPAN mirror is not used.

ZIP extraction elsewhere in `margin-data` still only checks archive safety. It
does not, by itself, interpret a member as a margin.
