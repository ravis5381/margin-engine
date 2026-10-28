# Limitations

This build does not replicate NSE Clearing or ICCL.

- NSE can download eight verified equity-derivatives reports and store the
  original bytes. The `.spn` member is parsed as XML-based SPAN 4.00. Scan
  points can be stored. Risk-array amounts stay in the file because it has no
  lot size or index/stock class. The exposure file's total applicable ELM
  percent is applied as stock membership by the margin quote. The contract-wise
  ELM file supplies the extreme-loss percent.
- No contract master has been ingested. SPAN scenario count, sign convention,
  and scale are not taken from the archived XML.
- `POST /api/v1/margin/calculate` returns SPAN scanning risk, the calendar-spread
  charge, extreme loss margin, option premium, delivery margin, and a
  cross-margin benefit of zero for NSE futures, options, and physical stocks.
  A book may contain more than one expiry. An index contract expiring on the
  trade date is not offset. Far-month futures in a spread are charged one third
  of the contract file's percent. Delivery margin is zero outside four weekdays
  before a stock expiry and is rejected inside that window because capital-market
  VaR is not archived. Cross-margin benefit is zero. Index-constituent
  cross margin is left out. Reported amounts are rounded half away from zero
  to paise. A monthly symbol with more than one futures expiry is rejected.
- The diagnostic worst loss is not a margin and is not returned by the API.
- `/ready` is unavailable because official calculation is not implemented.
  A synthetic dataset does not satisfy the official-dataset check.
- Example NSE/NSCCL and BSE/ICCL rows are operator configuration. They are not
  a finding that every position uses that arrangement, and they do not permit
  cross-exchange offsets.
- `www.nseindia.com` is the verified download host for the reports listed in
  `supported_formats.md`. ICCL's form host was not added, because no file was
  downloaded from it.
- Validation, reconciliation, and replay commands are not operational. NSE
  ingest archives bytes and does not calculate a margin. ICCL ingest is not
  operational.
- Ingest, contract, and reconciliation routes are absent. The margin route is
  published.
- Authentication is not implemented because those mutating routes are absent.
- Historical replay against an archived dataset is not implemented.
- There is no production-equivalence status on a calculation record.

The next implementation step is a contract master that can carry lot size, so
the checked SPAN risk-array values can be stored against contracts. A non-zero
short-option minimum and capital-market VaR are not in the archived files.
An ICCL file response is still required before that
adapter can be enabled. A draft dataset is not published as the official
calculation.
