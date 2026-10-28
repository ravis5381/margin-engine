# Risk-data operations

## Configuration

`MARGIN_CONFIG` and `MARGIN_DATA_SOURCES` point at TOML files. If those
variables are unset, the process reads `config/app.toml` and
`config/data_sources.toml`. A missing default file means that section is empty:
no clearing arrangement is authorized and no download host is approved.

`DATABASE_URL` comes from the environment. It is not printed by `AppConfig`'s
debug output. Example files contain placeholders only.

A disabled source must record a blocker. An enabled source must have an HTTPS
URL on an approved host, port 443, and no userinfo. Redirects are limited to
approved HTTPS hosts. The example NSE source points at
`https://www.nseindia.com/api/reports` and is enabled. The example ICCL source
points at the BSE risk-parameter listing and is enabled.

## Dataset versions

Each dataset records exchange, clearing corporation, trading date, snapshot
class, optional publication time, download time, filename, SHA-256, format,
parser version, contract-master version, validation status, and publication
status. `synthetic` datasets cannot carry a source host. `official` datasets
must. A published dataset must have passed validation and must name a contract
master.

`latest_validated` considers one exchange, one clearing corporation, one
snapshot class, and the exact trading date. Intraday sequence is ranked before
timestamp. If the newest rank is not published and passed, older files for that
date are not used. A repeated SHA-256 is a duplicate, not a new version.

## Commands

```text
margin-engine serve
margin-engine list-datasets
margin-engine ingest --source NSE --date 2026-10-27 --kind span-end-of-day
margin-engine ingest --source NSE --date 2026-10-27 --kind exposure-limit
margin-engine ingest --source NSE --date 2026-10-27 --kind contract-elm
margin-engine ingest --source ICCL --date 2026-10-28
margin-engine validate-dataset --dataset-id <uuid>
margin-engine reconcile --reference reference.json
margin-engine replay --input portfolio.json --date 2026-10-28
```

`serve`, `list-datasets`, and NSE `ingest` perform their operation. A margin
quote for a date with no archived SPAN file downloads that date's verified
reports before it calculates: each published SPAN snapshot, the exposure file,
the contract-wise ELM file, and the end-of-day cash-market VaR file from
`nsearchives.nseindia.com`. A snapshot that NSE has not published is skipped.
The request timeout in the example configuration is 180 seconds so those
downloads can finish. NSE ingest
downloads one verified report and stores the original bytes. A SPAN file becomes
a draft dataset. Exposure and contract-ELM files become auxiliary rows. The
command prints the id, filename, and SHA-256, and states that the file is not a
margin. A repeated SHA-256 reuses the existing row.

A missing `--kind`, an unknown kind, or a bad date exits 2. ICCL ingest
downloads the published SPAN snapshots and `ICCL_EDXELMddmmyy.zip`.
A disabled source, a failed download, a container mismatch, or a database
failure exits 1. Without `DATABASE_URL`, NSE ingest and `list-datasets` fail
and `/ready` is not ready. `validate-dataset` and `replay` still exit 3 after
argument checks.

`reconcile --reference` reads an operator JSON file. The file names the trade
time, `span_filename` or `span_sha256`, the positions, and the expected
`span_margin`, `extreme_loss_margin`, `premium`, `delivery_margin`, and
`total_margin`. The command quotes that book on the SPAN snapshot current at
the trade time. The quoted file must be the named file. Each component is
printed with its absolute difference. Any non-zero difference exits 1. A match
exits 0 and still prints `equivalence_claim=none`. A bad reference exits 2.

Set `MARGIN_ENGINE_SKIP_DOTENV=1` when a local `.env` must not affect a process.
