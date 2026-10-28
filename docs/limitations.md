# Limitations

This build does not replicate NSE Clearing or ICCL.

- NSE can download eight verified equity-derivatives reports and store the
  original bytes. The `.spn` member is parsed as XML-based SPAN 4.00. Scan
  points can be stored. Risk-array amounts stay in the file because it has no
  lot size or index/stock class. The exposure file's total applicable ELM
  percent is applied as stock membership by the margin quote. The contract-wise
  ELM file supplies the extreme-loss percent.
- The NSE F&O contract master supplies lot size. Quantity stays in units, so
  the lot size is reported and is not a hidden conversion. SPAN scenario count,
  sign convention, and scale are read when the quote parses the archived XML.
  The risk-array amounts stay in that file.
- `POST /api/v1/margin/calculate` takes a trade datetime and uses the latest
  archived SPAN snapshot whose `<created>` time is at or before that moment.
  The clock is India Standard Time. Exposure, contract ELM, cash VaR, and the
  contract master stay the files for that calendar date. The response is SPAN scanning risk, the calendar-spread
  charge, extreme loss margin, option premium, delivery margin, and a
  cross-margin benefit of zero for NSE futures, options, and physical stocks.
  A book may contain more than one expiry. An index contract expiring on the
  trade date is not offset. Far-month futures in a spread are charged one third
  of the contract file's percent. Delivery margin on a stock derivative inside
  four weekdays of expiry is the published fraction of the EQ daily margin rate
  in an archived cash-market VaR file, on the net shares that expiry would
  deliver. It is zero outside that window and for an index. Without that file
  the window is rejected. Cross-margin benefit is zero. Index-constituent
  cross margin is left out. Reported amounts are rounded half away from zero
  to paise. The contract file identifies the expiry. An ambiguous symbol must
  be sent as a canonical contract such as `NSE:RELIANCE:2026-11-26:FUT`, or as
  underlying `RELIANCE`, expiry `2026-11-26`, and right `FUT`. An empty book, a
  zero quantity, a repeated symbol, a non-positive option strike, and a
  position that sends both a symbol and those fields are rejected before the
  archive is read. `POST /api/v1/instruments/canonical` returns that symbol
  from the same fields and does not calculate a margin.
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

A non-zero short-option minimum is rejected when the SPAN file publishes one.
The archived files so far publish a zero rate. A clearing-corporation margin
reference is still required before this quote is an official calculation.
An ICCL file response is still required before that
adapter can be enabled. A draft dataset is not published as the official
calculation.
