# Calculation methodology

## What is implemented

For a non-empty book whose normalized risk arrays use the same scenario ids in
the same order, the diagnostic is:

- `P_s = sum(q_i * R_i,s)`
- `L_s = -P_s`
- worst-loss diagnostic `= max(0, max L_s)`

`q_i` is the signed quantity in the array's quantity unit. `R_i,s` is canonical
P&L per unit. A source value marked as a loss is negated once, then multiplied
once by its positive scale. The original source token is kept.

An empty book returns no worst-loss amount. A book that is profitable in every
scenario has a worst-loss diagnostic of zero. That zero is not a SPAN margin.
`scanning_risk` still returns an error.

Zero-quantity positions are omitted. Duplicate positions with the same identity
and the same unit, origin, and prices are summed. Conflicting attributes are
rejected. Positions on different exchanges, or cleared by different
corporations, cannot share one calculation scope. Lot size is not applied as a
hidden conversion.

`POST /api/v1/margin/calculate` reads futures, option, and physical-stock risk
arrays from the newest archived NSE SPAN file for the requested date, and it
reads the exposure file archived for that same date. A risk-array loss is
positive for a long unit. Scanning risk is the greatest scenario loss, and it
is zero when every scenario is a gain or the array is zero. Quantity is in
units. Monthly symbols are `PRODUCTYYMMMFUT` and `PRODUCTYYMMMSTRIKECE` or
`PE`. Dated symbols are `PRODUCTYYMDDFUT` and `PRODUCTYYMDDSTRIKECE` or `PE`,
with month codes `1`–`9`, `O`, `N`, and `D`. A physical stock is the product
code. A monthly symbol that matches more than one expiry is rejected unless
one futures expiry in that month identifies the monthly option. Offsetting
expiries add the flat calendar-spread charge published in the SPAN file. An
index contract expiring on the trade date is scanned on its own and is not
part of that spread. The short-option minimum is zero when the SPAN file's
rate is a single zero; any other rate is rejected for a short option.

Extreme loss margin uses the percent in the contract-wise ELM file. When that
file is absent, a unit test can still use the exposure file and the index rates
published by NSE Clearing. A futures calendar spread charges the far-month
quantity at one third of its percent and the unmatched quantity at the full
percent. The near-month matched quantity is zero. A short option is charged
`abs(quantity) * SPAN underlying price * percent / 100`. A long option and a
physical stock are charged zero. A short index option's published schedule,
used only when the contract file is empty, is 2 percent, 3 percent when the
strike is more than 10 percent out of the money against the SPAN underlying
price, or 5 percent when expiry is more than nine months after the trade date,
plus 2 percent on the expiry day. If both the 3 percent case and the 5 percent
case apply, the quote is rejected.

Option premium is `quantity * SPAN option price`. A short option's premium is
negative. Payable premium is the positive net premium of the book. Delivery
margin is zero for an index and for a stock derivative whose expiry is more
than four weekdays away. Inside that window the quote is rejected because the
capital-market VaR rate is not archived. The count uses weekdays and has no
holiday calendar. Cross-margin benefit is zero. Index-constituent cross
margin is left out. SPAN margin is scanning risk plus the spread charge plus
the short-option minimum. The reported total is that SPAN margin plus exposure
margin. A short option's premium is included in the total. A book of only long
options has a total of zero. Margin benefit is the amount by which a spread
total is below the sum of the separate position totals, and that reduction is
already in the total.
Each reported amount is rounded half away from zero to paise. No NSE rounding
circular was found; the rounding unit is the INR minor unit.

## What is not implemented

The diagnostic still reads arrays supplied by the caller and is not the margin
route. A configuration flag set to permit a cross-exchange offset does not
create a benefit. Delivery margin inside the four weekdays before a stock
expiry is not calculated, because the capital-market VaR rate is not in the
archived files. Index-constituent cross margin is left out, so the
cross-margin benefit is zero.

Missing risk arrays, mismatched scenario order, mismatched quantity units, and
unusable datasets produce errors. The engine does not substitute an older file
or another trade date. When several files share the newest rank and their
hashes differ, selection fails.
