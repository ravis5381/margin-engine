# Margin engine

Rust workspace for an exchange-aware Indian equity-derivatives margin calculator.
This build covers the domain model, configuration, dataset lifecycle, a diagnostic
scenario sum, and the health API.

It does not replicate NSE Clearing or ICCL margin calculations. Verified NSE
SPAN, exposure, and contract-ELM files can be downloaded and archived. The SPAN
member is parsed as XML-based SPAN 4.00. The margin quote returns scanning
risk, the calendar-spread charge, extreme loss margin from the contract-wise
file, option premium, and delivery margin. Far-month futures in a spread are
charged one third of the contract percent. Cross-margin benefit is zero.
Reported amounts are rounded half away
from zero to paise. Risk-array amounts stay in the archived SPAN file.

## Layout

| Path | Responsibility |
| --- | --- |
| `crates/margin-domain` | Exchanges, contracts, money, positions, datasets, risk arrays |
| `crates/margin-data` | Configuration, NSE report download, HTTPS allowlist, ZIP checks, PostgreSQL archive |
| `crates/margin-span` | Diagnostic scenario aggregation and fail-closed SPAN components |
| `crates/margin-rules` | Versioned rule records that do not calculate charges |
| `crates/margin-reconciliation` | Absolute and relative differences |
| `crates/margin-api` | Health, readiness, and dataset metadata |
| `migrations` | PostgreSQL schema |
| `docs` | Architecture, formats, methodology, operations, limits |

The binary is `margin-engine`.

## Run

Copy `.env.example` to `.env` and `config/*.example.toml` to `config/app.toml` and
`config/data_sources.toml` before using a real database. The sample NSE reports
source is enabled. The sample ICCL source stays disabled. `DATABASE_URL` is
read only from the environment.

```text
cargo test --workspace
cargo run -- serve
cargo run -- list-datasets
cargo run -- ingest --source NSE --date 2026-10-27 --kind span-end-of-day
```

NSE `ingest` downloads one verified report and stores the original bytes. It
does not calculate a margin. ICCL `ingest`, `validate-dataset`, `reconcile`,
and `replay` validate their arguments and then exit 3.

`GET /health` is a liveness check. `GET /ready` stays unavailable until a
published official dataset exists and an official calculation is implemented.
Both conditions are required. This build implements neither the official
calculation nor a verified dataset.

Swagger UI is served at `/swagger-ui`. The OpenAPI document is
`GET /api/v1/openapi.json`. Both describe the routes that exist. They do not
list margin calculation, ingestion, or reconciliation operations.

## Database

`docker compose up postgres` starts PostgreSQL 16. The compose password is for
local development. Migrations run when `margin-engine` connects.

## Status

See [docs/limitations.md](docs/limitations.md). Equivalence with NSE Clearing or
ICCL is not claimed.
