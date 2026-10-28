# Reconciliation

`compare` stores the expected amount, the calculated amount, the absolute
difference, and a relative difference when the expected amount is not zero.
Relative difference is absent when the expected amount is zero, so a non-zero
calculated amount is outside tolerance in that case.

`material` is true whenever the absolute difference is not zero. A tolerance
can mark `within_diagnostic_tolerance` without clearing `material`.

The caller supplies the diagnostic category. The comparison does not infer
wrong contract mapping, a missing risk array, spread eligibility, option
valuation, ELM, delivery margin, aggregation, rounding, snapshot selection, or
a missing rule.

Resolution starts unresolved. It can move once to explained or accepted
exception, and the note is required. The material flag stays.

No official reference file is loaded, and no calculation is marked equivalent
to NSE Clearing or ICCL.
