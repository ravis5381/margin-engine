# Architecture

The workspace separates identity and arithmetic from I/O.

`margin-domain` owns exchanges (`NSE`, `BSE`), clearing corporations (`NSCCL`,
`ICCL`), contracts, quantities, money, positions, dataset versions, and
normalized risk arrays. `NSCCL` is the external code used for NSE Clearing
Limited. A clearing relationship comes from configuration. An empty
configuration authorizes no exchange and clearing-corporation pair.

`margin-data` loads that configuration, checks HTTPS hosts, downloads the
verified NSE reports, and stores original file bytes. A `.spn` member is parsed
as XML-based SPAN 4.00. Scan points from a single rate are stored with the
draft dataset. Risk-array amounts remain in the archived file. The margin quote
reads the contract-wise ELM percent and an archived cash-market VaR rate for
delivery margin, applies the SPAN calendar-spread charge,
and rounds each reported amount half away from zero to paise. Domain addition
does not round. There is no foreign-exchange conversion.

`margin-span` can sum a normalized portfolio scenario. That function is a
diagnostic. Scanning risk, spreads, the short-option minimum, and net option
value return errors.

`margin-rules` can select a rule record by effective date. Applying the record
returns an error. ELM, delivery margin, premium, crystallised obligations,
additional margin, and cross-margin benefit do the same.

`margin-reconciliation` compares two amounts. A difference stays material when
the amounts differ, including when it is inside a diagnostic tolerance.

`margin-api` exposes liveness, readiness, data status, and dataset metadata.
Swagger UI at `/swagger-ui` and the OpenAPI document at `/api/v1/openapi.json`
describe those routes, including margin calculation. Ingestion and
reconciliation routes are not registered.

PostgreSQL is the system of record for archived bytes and published dataset
versions. Publication is an insert. A published row can move only to
superseded, and the other columns stay unchanged. Source-file rows cannot be
updated or deleted.

Money uses `rust_decimal::Decimal` with an explicit `INR` currency. Addition
does not round. The margin quote rounds each reported amount half away from
zero to paise. There is no foreign-exchange conversion.
