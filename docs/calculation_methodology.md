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

`POST /api/v1/margin/calculate` takes a trade datetime. A time without an
offset is India Standard Time, the same clock as the SPAN `<created>` value.
The quote uses the latest archived NSE SPAN snapshot whose created time is at
or before that moment, and it reads the exposure, contract ELM, cash VaR, and
contract master files for that calendar date. A datetime before the first
published snapshot of the day is rejected. Futures, option, and physical-stock
risk arrays come from that SPAN file. A risk-array loss is
positive for a long unit. Scanning risk is the greatest scenario loss, and it
is zero when every scenario is a gain or the array is zero. Quantity is in
units. Monthly symbols are `PRODUCTYYMMMFUT` and `PRODUCTYYMMMSTRIKECE` or
`PE`. Dated symbols are `PRODUCTYYMDDFUT` and `PRODUCTYYMDDSTRIKECE` or `PE`,
with month codes `1`–`9`, `O`, `N`, and `D`. A physical stock is the product
code. The contract-wise ELM file identifies the contract. Each returned
line includes a canonical symbol built from the exchange, underlying, expiry,
and instrument type: `NSE:PRODUCT:YYYY-MM-DD:FUT`, `NSE:PRODUCT:YYYY-MM-DD:STRIKE:CE`,
`NSE:PRODUCT:YYYY-MM-DD:STRIKE:PE`, or `NSE:PRODUCT:EQ`. A request may send
those parts as `underlying`, `expiry`, `strike`, and `instrumentType` (`FUT`, `C`, `P`,
`CE`, `PE`, or `EQ`) and the route builds that symbol before the SPAN match.
A position sends either that symbol or those fields. An empty book, a zero
quantity, a repeated symbol, a non-positive option strike, and a position that
sends both are rejected before the archive is read.
A symbol that matches more than one row is rejected, and the error lists those
canonical symbols. The same symbol may be sent in place of a market symbol.
When the contract
file is not loaded, a monthly symbol that matches more than one expiry is
rejected unless one futures expiry in that month identifies the monthly
option. Offsetting
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
margin applies to a stock derivative from four weekdays before expiry through
the expiry day. The quantity is the net shares that expiry would deliver:
futures and in-the-money options in that expiry are signed by whether they
receive or deliver the stock, and out-of-the-money options contribute nothing.
In-the-money uses the SPAN underlying price. The charge is that quantity times
the underlying price times the EQ daily margin rate in the archived
cash-market VaR file, times 10 percent on the fourth weekday before expiry,
25 percent on the third, 45 percent on the second, 70 percent on the weekday
before expiry, and 100 percent on the expiry day. An index and a position
outside that window are zero. The count uses weekdays and has no holiday
calendar. Mark-to-market is not a rate in the VaR file, so it is not added.
If the window applies and no VaR file is archived, the quote is rejected.
Cross-margin benefit is zero. Index-constituent cross margin is left out.
SPAN margin is scanning risk plus the spread charge plus the short-option
minimum. The reported total is that SPAN margin plus exposure margin plus
delivery margin. A short option's premium is included in the total. A book of
only long options has a total equal to its delivery margin. Margin benefit is
the amount by which a spread total is below the sum of the separate position
totals, measured before delivery margin, and that reduction is already in the
total.
Each reported amount is rounded half away from zero to paise. No NSE rounding
circular was found; the rounding unit is the INR minor unit.

## What is not implemented

The diagnostic still reads arrays supplied by the caller and is not the margin
route. A configuration flag set to permit a cross-exchange offset does not
create a benefit. Index-constituent cross margin is left out, so the
cross-margin benefit is zero. A cash-market VaR file that is not archived
leaves delivery margin inside the expiry window uncalculated.

Missing risk arrays, mismatched scenario order, mismatched quantity units, and
unusable datasets produce errors. The engine does not substitute an older file
or another trade date. When several files share the newest rank and their
hashes differ, selection fails.
