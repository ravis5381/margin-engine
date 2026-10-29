use super::{
    CashMarginRates, ContractElmRates, ContractKind, ExposureRates, MarginPosition, QuoteBuilder,
    SpanMarginQuote, parse_span_index, quote_span_xml,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 28).expect("date")
}

fn position(symbol: &str, quantity: &str) -> MarginPosition {
    MarginPosition {
        symbol: symbol.to_owned(),
        quantity: Decimal::from_str_exact(quantity).expect("qty"),
    }
}

fn amount(text: &str) -> Decimal {
    Decimal::from_str_exact(text).expect("amount")
}

fn exposure() -> ExposureRates {
    ExposureRates::parse_csv(
        b"Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date 28-Oct-2026,Total applicable ELM%\n1,RELIANCE,OTH,3.5,0,3.5\n2,RELIANCE,OTM,5.25,0,5.25\n",
        date(),
    )
    .expect("exposure")
}

fn value_book(xml: &[u8], positions: &[MarginPosition]) -> Result<SpanMarginQuote, String> {
    quote_span_xml(
        xml,
        date(),
        positions,
        &exposure(),
        &ContractElmRates::default(),
        &CashMarginRates::default(),
    )
}

fn document() -> String {
    r#"<?xml version="1.0"?>
<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<futPf><pfCode>NIFTY</pfCode>
<fut><pe>20261123</pe><p>22807.60</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>8.56</a><a>-2113.07</a><a>2130.20</a></ra>
</fut>
<fut><pe>20261229</pe><p>22951.50</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>1</a><a>2</a><a>3</a></ra>
</fut>
</futPf>
<ccDef><cc>NIFTY</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth></ccDef>
</spanFile>"#
        .to_owned()
}

fn book() -> String {
    r#"<?xml version="1.0"?>
<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<futPf><pfCode>NIFTY</pfCode>
<fut><pe>20261123</pe><p>22807.60</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>8.56</a><a>-2113.07</a><a>2130.20</a></ra>
</fut>
</futPf>
<oopPf><pfCode>NIFTY</pfCode>
<series><pe>20261103</pe><cvf>1.00</cvf><sc>1.00000</sc>
<opt><o>C</o><k>26000</k><p>10</p>
<ra><r>1</r><a>1</a><a>4</a><a>-2</a></ra>
</opt>
</series>
<series><pe>20261123</pe><cvf>1.00</cvf><sc>1.00000</sc>
<opt><o>C</o><k>26000</k><p>50</p>
<ra><r>1</r><a>0</a><a>-5.79</a><a>3</a></ra>
</opt>
<opt><o>P</o><k>26000</k><p>40</p>
<ra><r>1</r><a>2</a><a>0</a><a>-1</a></ra>
</opt>
</series>
</oopPf>
<phyPf><pfCode>NIFTY</pfCode>
<phy><pe>00000000</pe><p>22706.50</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>0</a><a>0</a><a>0</a></ra>
</phy>
</phyPf>
<futPf><pfCode>RELIANCE</pfCode>
<fut><pe>20261123</pe><p>1171</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>1</a><a>2</a><a>3</a></ra>
</fut>
<fut><pe>20261126</pe><p>1172</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>0.44</a><a>-55.05</a><a>166.92</a></ra>
</fut>
</futPf>
<oopPf><pfCode>RELIANCE</pfCode>
<series><pe>20261126</pe><cvf>1</cvf><sc>1</sc>
<opt><o>C</o><k>1400</k><p>12.5</p>
<ra><r>1</r><a>0.31</a><a>-54.85</a><a>55.48</a></ra>
</opt>
</series>
</oopPf>
<phyPf><pfCode>RELIANCE</pfCode>
<phy><pe>00000000</pe><p>1166.90</p><cvf>1.00</cvf><sc>1.00000</sc>
<ra><r>1</r><a>0.00</a><a>0.00</a><a>-0.00</a></ra>
</phy>
</phyPf>
<ccDef><cc>NIFTY</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0</val></rate></tier></somTiers>
</ccDef>
<ccDef><cc>RELIANCE</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0.00</val></rate></tier></somTiers>
</ccDef>
</spanFile>"#
        .to_owned()
}

#[test]
fn short_november_future_uses_the_greatest_loss() {
    let quote =
        value_book(document().as_bytes(), &[position("NIFTY26NOVFUT", "-65")]).expect("quote");
    assert_eq!(quote.span_margin, amount("137349.55"));
    assert_eq!(quote.scanning_risk, quote.span_margin);
    assert_eq!(quote.spread_charge, Decimal::ZERO);
    assert_eq!(quote.short_option_minimum, Decimal::ZERO);
    assert_eq!(quote.extreme_loss_margin, amount("29649.88"));
    assert_eq!(quote.lines[0].elm_percent, amount("2"));
    assert_eq!(quote.premium, Decimal::ZERO);
    assert_eq!(quote.premium_margin, Decimal::ZERO);
    assert_eq!(quote.total_margin, amount("166999.43"));
    assert_eq!(quote.lines[0].instrument, "future");
    assert_eq!(quote.lines[0].strike, None);
    assert_eq!(quote.lines[0].worst_scan_points, vec![2]);
    assert_eq!(
        quote.lines[0].expiry,
        Some(NaiveDate::from_ymd_opt(2026, 11, 23).expect("expiry"))
    );
    assert_eq!(quote.lines[0].settlement_price, amount("22807.60"));
}

#[test]
fn a_long_future_uses_its_greatest_loss_and_a_missing_contract_fails() {
    let quote =
        value_book(document().as_bytes(), &[position("NIFTY26NOVFUT", "65")]).expect("quote");
    assert_eq!(quote.span_margin, amount("138463"));
    assert_eq!(quote.extreme_loss_margin, amount("29649.88"));
    assert_eq!(quote.total_margin, amount("168112.88"));
    assert!(value_book(document().as_bytes(), &[position("NIFTY26OCTFUT", "1")]).is_err());
    let two = document().replace("20261229", "20261124");
    assert!(value_book(two.as_bytes(), &[position("NIFTY26NOVFUT", "1")]).is_err());
}

#[test]
fn options_stocks_and_dated_stock_futures_use_their_risk_arrays() {
    let monthly = value_book(
        book().as_bytes(),
        &[
            position("NIFTY26NOV26000CE", "-1"),
            position("NIFTY26NOV26000PE", "1"),
        ],
    )
    .expect("same expiry");
    assert_eq!(monthly.lines[0].instrument, "call");
    assert_eq!(monthly.lines[0].scanning_risk, amount("5.79"));
    assert_eq!(monthly.lines[0].extreme_loss_margin, amount("681.20"));
    assert_eq!(monthly.lines[0].elm_percent, amount("3"));
    assert_eq!(monthly.lines[0].premium, amount("-50"));
    assert_eq!(monthly.lines[1].instrument, "put");
    assert_eq!(monthly.lines[1].extreme_loss_margin, Decimal::ZERO);
    assert_eq!(monthly.lines[1].premium, amount("40"));
    assert_eq!(monthly.premium, amount("-10"));
    assert_eq!(monthly.premium_margin, Decimal::ZERO);
    assert_eq!(monthly.span_margin, amount("5.79"));
    assert_eq!(monthly.total_margin, amount("696.99"));
    assert!(monthly.note.contains("short-option minimum rate"));
    assert_eq!(
        monthly.lines[0].expiry,
        Some(NaiveDate::from_ymd_opt(2026, 11, 23).expect("monthly"))
    );
    assert_eq!(monthly.lines[1].scanning_risk, amount("2"));

    let weekly =
        value_book(book().as_bytes(), &[position("NIFTY26N0326000CE", "-1")]).expect("weekly");
    assert_eq!(weekly.lines[0].scanning_risk, amount("2"));
    assert_eq!(weekly.lines[0].extreme_loss_margin, amount("681.20"));
    assert_eq!(
        weekly.lines[0].expiry,
        Some(NaiveDate::from_ymd_opt(2026, 11, 3).expect("weekly"))
    );

    let stock = value_book(book().as_bytes(), &[position("RELIANCE", "5")]).expect("stock");
    assert_eq!(stock.lines[0].instrument, "stock");
    assert_eq!(stock.lines[0].expiry, None);
    assert_eq!(stock.lines[0].scanning_risk, Decimal::ZERO);
    assert_eq!(stock.lines[0].extreme_loss_margin, Decimal::ZERO);
    assert_eq!(stock.lines[0].settlement_price, amount("1166.90"));
    assert_eq!(stock.total_margin, Decimal::ZERO);

    let future =
        value_book(book().as_bytes(), &[position("RELIANCE26N26FUT", "-1")]).expect("dated future");
    assert_eq!(future.lines[0].instrument, "future");
    assert_eq!(future.lines[0].scanning_risk, amount("55.05"));
    assert_eq!(future.lines[0].extreme_loss_margin, amount("41.02"));
    assert_eq!(future.lines[0].elm_percent, amount("3.5"));
    assert_eq!(future.total_margin, amount("96.07"));
    assert_eq!(
        future.lines[0].expiry,
        Some(NaiveDate::from_ymd_opt(2026, 11, 26).expect("day"))
    );

    let option = value_book(book().as_bytes(), &[position("RELIANCE26N261400CE", "-2")])
        .expect("stock option");
    assert_eq!(option.lines[0].instrument, "call");
    assert_eq!(option.lines[0].strike, Some(amount("1400")));
    assert_eq!(option.lines[0].scanning_risk, amount("109.70"));
    assert_eq!(option.lines[0].underlying_price, Some(amount("1166.90")));
    assert_eq!(option.lines[0].elm_percent, amount("3.5"));
    assert_eq!(option.lines[0].extreme_loss_margin, amount("81.68"));
    assert_eq!(option.lines[0].premium, amount("-25"));
    assert_eq!(option.premium_margin, Decimal::ZERO);
    assert_eq!(option.total_margin, amount("216.38"));

    let ambiguous = value_book(book().as_bytes(), &[position("RELIANCE26NOVFUT", "1")])
        .expect_err("two November futures");
    assert!(ambiguous.contains("RELIANCE26N23FUT"), "{ambiguous}");
    assert!(ambiguous.contains("RELIANCE26N26FUT"), "{ambiguous}");
}

#[test]
fn a_monthly_option_without_one_futures_expiry_is_rejected() {
    let xml = book().replacen("<fut><pe>20261123</pe>", "<fut><pe>20261223</pe>", 1);
    let error = value_book(xml.as_bytes(), &[position("NIFTY26NOV26000CE", "-1")])
        .expect_err("ambiguous month");
    assert!(error.contains("NIFTY26N0326000CE"));
    assert!(error.contains("NIFTY26N2326000CE"));
    let priced = book().replace("<val>0</val>", "<val>5</val>");
    assert!(value_book(priced.as_bytes(), &[position("NIFTY26NOV26000CE", "-1")]).is_err());
    let long =
        value_book(priced.as_bytes(), &[position("NIFTY26NOV26000CE", "1")]).expect("long option");
    assert_eq!(long.short_option_minimum, Decimal::ZERO);
    assert_eq!(long.lines[0].scanning_risk, amount("3"));
    assert_eq!(long.lines[0].extreme_loss_margin, Decimal::ZERO);
    assert_eq!(long.premium, amount("50"));
    assert_eq!(long.premium_margin, amount("50"));
    assert_eq!(long.total_margin, Decimal::ZERO);
}

#[test]
fn index_short_option_rates_follow_the_published_schedule() {
    let expiry_day = book()
        .replace("<pe>20261103</pe>", "<pe>20261028</pe>")
        .replace("<k>26000</k><p>10</p>", "<k>22000</k><p>10</p>");
    let on_expiry = value_book(
        expiry_day.as_bytes(),
        &[position("NIFTY26O2822000CE", "-1")],
    )
    .expect("expiry day");
    assert_eq!(on_expiry.lines[0].elm_percent, amount("4"));
    assert_eq!(on_expiry.extreme_loss_margin, amount("908.26"));

    let far = book()
        .replace("<pe>20261103</pe>", "<pe>20270729</pe>")
        .replace("<k>26000</k><p>10</p>", "<k>22000</k><p>10</p>");
    let long_dated =
        value_book(far.as_bytes(), &[position("NIFTY2772922000CE", "-1")]).expect("far");
    assert_eq!(long_dated.lines[0].elm_percent, amount("5"));

    let both = book()
        .replace("<pe>20261103</pe>", "<pe>20270729</pe>")
        .replace("<k>26000</k><p>10</p>", "<k>30000</k><p>10</p>");
    let error = value_book(both.as_bytes(), &[position("NIFTY2772930000CE", "-1")])
        .expect_err("two index rates");
    assert!(error.contains("neither published index rate"), "{error}");
}

#[test]
fn stock_extreme_loss_uses_the_exposure_file_total() {
    let deep = book().replace("<k>1400</k>", "<k>2000</k>");
    let option =
        value_book(deep.as_bytes(), &[position("RELIANCE26N262000CE", "-1")]).expect("deep");
    assert_eq!(option.lines[0].elm_percent, amount("5.25"));
    assert_eq!(option.extreme_loss_margin, amount("61.26"));

    let raised = ExposureRates::parse_csv(
        b"Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date 28-Oct-2026,Total applicable ELM%\n1,RELIANCE,OTH,3.5,15,18.5\n2,RELIANCE,OTM,5.25,15,20.25\n",
        date(),
    )
    .expect("raised");
    let future = quote_span_xml(
        book().as_bytes(),
        date(),
        &[position("RELIANCE26N26FUT", "-1")],
        &raised,
        &ContractElmRates::default(),
        &CashMarginRates::default(),
    )
    .expect("additional");
    assert_eq!(future.lines[0].elm_percent, amount("18.5"));
    assert_eq!(future.extreme_loss_margin, amount("216.82"));

    let mismatch = ExposureRates::parse_csv(
        b"Sr No.,Symbol,Instrument Type,Normal ELM Margin%,Additional ELM% for Trade Date 28-Oct-2026,Total applicable ELM%\n1,RELIANCE,OTH,3.5,0,4\n2,RELIANCE,OTM,5.25,0,5.25\n",
        date(),
    );
    assert!(mismatch.is_err());
}

#[test]
fn a_short_option_without_an_underlying_price_is_rejected() {
    let xml = book().replace(
        "<phyPf><pfCode>NIFTY</pfCode>\n<phy><pe>00000000</pe><p>22706.50</p><cvf>1</cvf><sc>1</sc>\n<ra><r>1</r><a>0</a><a>0</a><a>0</a></ra>\n</phy>\n</phyPf>\n",
        "",
    );
    let error =
        value_book(xml.as_bytes(), &[position("NIFTY26N0326000CE", "-1")]).expect_err("price");
    assert!(error.contains("underlying price"), "{error}");
}

fn spread_file(near_expiry: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<futPf><pfCode>NIFTY</pfCode>
<fut><pe>{near_expiry}</pe><p>100</p><d>1</d><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>10</a><a>-4</a></ra></fut>
<fut><pe>20261229</pe><p>110</p><d>1</d><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>20</a><a>-8</a></ra></fut>
</futPf>
<ccDef><cc>NIFTY</cc><riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0</val></rate></tier></somTiers>
</ccDef>
<dSpread>
<spread>1</spread><chargeMeth>F</chargeMeth><rate><r>1</r><val>5</val></rate>
<pLeg><cc>NIFTY</cc><pe>{near_expiry}</pe><rs>A</rs><i>1</i></pLeg>
<pLeg><cc>NIFTY</cc><pe>20261229</pe><rs>B</rs><i>1</i></pLeg>
</dSpread>
<dSpread><spread></spread><chargeMeth></chargeMeth><rate><r></r><val></val></rate></dSpread>
</spanFile>"#
    )
}

#[test]
fn a_futures_calendar_spread_charges_the_file_and_one_third_of_the_far_month() {
    let positions = [
        position("NIFTY26NOVFUT", "2"),
        position("NIFTY26DECFUT", "-2"),
    ];
    let quote = value_book(spread_file("20261123").as_bytes(), &positions).expect("spread");
    assert_eq!(quote.scanning_risk, amount("8"));
    assert_eq!(quote.spread_charge, amount("10"));
    assert_eq!(quote.span_margin, amount("18"));
    assert_eq!(quote.lines[0].extreme_loss_margin, Decimal::ZERO);
    assert_eq!(quote.lines[1].extreme_loss_margin, amount("1.47"));
    assert_eq!(quote.lines[1].elm_percent, amount("2"));
    assert_eq!(quote.delivery_margin, Decimal::ZERO);
    assert_eq!(quote.cross_margin_benefit, Decimal::ZERO);
    assert_eq!(quote.lines[0].total_margin, amount("24"));
    assert_eq!(quote.lines[1].total_margin, amount("20.40"));
    assert_eq!(quote.margin_benefit, amount("24.93"));
    assert_eq!(quote.total_margin, amount("19.47"));

    let held = value_book(
        book().as_bytes(),
        &[
            position("RELIANCE", "5"),
            position("RELIANCE26N26FUT", "-1"),
        ],
    )
    .expect("stock and future");
    assert_eq!(held.scanning_risk, amount("55.05"));
    assert_eq!(held.spread_charge, Decimal::ZERO);
    assert_eq!(held.total_margin, amount("96.07"));
}

#[test]
fn an_index_spread_does_not_offset_a_contract_expiring_today() {
    let positions = [
        position("NIFTY26OCTFUT", "2"),
        position("NIFTY26DECFUT", "-2"),
    ];
    let quote = value_book(spread_file("20261028").as_bytes(), &positions).expect("expiry");
    assert_eq!(quote.scanning_risk, amount("36"));
    assert_eq!(quote.spread_charge, Decimal::ZERO);
    assert_eq!(quote.extreme_loss_margin, amount("8.40"));
    assert_eq!(quote.span_margin, amount("36"));
}

#[test]
fn the_contract_file_supplies_the_extreme_loss_percent() {
    let rates = ContractElmRates::parse_csv(
        b"FUTIDX,NIFTY,23-NOV-2026,-1,XX,0,2.5\nOPTIDX,NIFTY,03-NOV-2026,26000,CE,0,3\n",
    )
    .expect("contracts");
    let quote = quote_span_xml(
        document().as_bytes(),
        date(),
        &[position("NIFTY26NOVFUT", "-65")],
        &exposure(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect("file rate");
    assert_eq!(quote.lines[0].canonical, "NSE:NIFTY:2026-11-23:FUT");
    assert_eq!(quote.lines[0].elm_percent, amount("2.5"));
    assert_eq!(quote.extreme_loss_margin, amount("37062.35"));
    let missing = quote_span_xml(
        document().as_bytes(),
        date(),
        &[position("NIFTY26DECFUT", "1")],
        &exposure(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect_err("missing contract");
    assert!(missing.contains("not in the contract file"), "{missing}");
}

#[test]
fn delivery_margin_uses_the_cash_rate_on_the_net_deliverable_shares() {
    let outside =
        value_book(book().as_bytes(), &[position("RELIANCE26N26FUT", "-1")]).expect("outside");
    assert_eq!(outside.delivery_margin, Decimal::ZERO);
    assert_eq!(outside.cross_margin_benefit, Decimal::ZERO);
    let inside = book().replace("<pe>20261126</pe>", "<pe>20261103</pe>");
    let missing = value_book(inside.as_bytes(), &[position("RELIANCE26N03FUT", "-1")])
        .expect_err("inside window");
    assert!(missing.contains("capital-market VaR"), "{missing}");

    let cash = CashMarginRates::parse(
        b"10,28102026,0.00,06,0000001\n20,RELIANCE,EQ,INE002A01018,8.55,0.00,9.00,3.50,0.00,12.50\n",
        date(),
    )
    .expect("var");
    let quote = quote_span_xml(
        inside.as_bytes(),
        date(),
        &[position("RELIANCE26N03FUT", "-1")],
        &exposure(),
        &ContractElmRates::default(),
        &cash,
    )
    .expect("delivery");
    assert_eq!(quote.delivery_margin, amount("14.59"));
    assert_eq!(quote.scanning_risk, amount("55.05"));
    assert_eq!(quote.extreme_loss_margin, amount("41.02"));
    assert_eq!(quote.total_margin, amount("110.66"));

    let otm = value_book(inside.as_bytes(), &[position("RELIANCE26N031400CE", "-2")]).expect("otm");
    assert_eq!(otm.delivery_margin, Decimal::ZERO);

    let offset = inside.replace("<o>C</o><k>1400</k>", "<o>P</o><k>1400</k>");
    let netted = value_book(
        offset.as_bytes(),
        &[
            position("RELIANCE26N03FUT", "1"),
            position("RELIANCE26N031400PE", "1"),
        ],
    )
    .expect("netted");
    assert_eq!(netted.delivery_margin, Decimal::ZERO);

    let expiry_day = book().replace("<pe>20261126</pe>", "<pe>20261028</pe>");
    let expiry = quote_span_xml(
        expiry_day.as_bytes(),
        date(),
        &[position("RELIANCE26O28FUT", "-1")],
        &exposure(),
        &ContractElmRates::default(),
        &cash,
    )
    .expect("expiry day");
    assert_eq!(expiry.delivery_margin, amount("145.86"));

    let other = CashMarginRates::parse(
        b"10,28102026,0.00,06,0000001\n20,HDFCBANK,EQ,INE040A01034,8.39,0.00,9.00,3.50,0.00,12.50\n",
        date(),
    )
    .expect("other");
    let absent = quote_span_xml(
        inside.as_bytes(),
        date(),
        &[position("RELIANCE26N03FUT", "-1")],
        &exposure(),
        &ContractElmRates::default(),
        &other,
    )
    .expect_err("missing symbol");
    assert!(absent.contains("no EQ series"), "{absent}");
}

#[test]
fn the_contract_file_identifies_the_expiry_and_a_canonical_symbol_selects_one() {
    let rates = ContractElmRates::parse_csv(
        b"FUTSTK,RELIANCE,23-NOV-2026,-1,XX,0,3.5\nFUTSTK,RELIANCE,26-NOV-2026,-1,XX,0,3.5\nFUTIDX,NIFTY,23-NOV-2026,-1,XX,0,2\nOPTIDX,NIFTY,03-NOV-2026,26000,CE,0,3\n",
    )
    .expect("contracts");
    let ambiguous = quote_span_xml(
        book().as_bytes(),
        date(),
        &[position("RELIANCE26NOVFUT", "1")],
        &exposure(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect_err("two expiries");
    assert!(
        ambiguous.contains("NSE:RELIANCE:2026-11-23:FUT"),
        "{ambiguous}"
    );
    assert!(
        ambiguous.contains("NSE:RELIANCE:2026-11-26:FUT"),
        "{ambiguous}"
    );

    let chosen = quote_span_xml(
        book().as_bytes(),
        date(),
        &[position("NSE:RELIANCE:2026-11-26:FUT", "-1")],
        &exposure(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect("canonical");
    assert_eq!(chosen.lines[0].canonical, "NSE:RELIANCE:2026-11-26:FUT");
    assert_eq!(
        chosen.lines[0].expiry,
        Some(NaiveDate::from_ymd_opt(2026, 11, 26).expect("expiry"))
    );

    let only_far = ContractElmRates::parse_csv(b"FUTSTK,RELIANCE,26-NOV-2026,-1,XX,0,3.5\n")
        .expect("one expiry");
    let resolved = quote_span_xml(
        book().as_bytes(),
        date(),
        &[position("RELIANCE26NOVFUT", "-1")],
        &exposure(),
        &only_far,
        &CashMarginRates::default(),
    )
    .expect("identified");
    assert_eq!(resolved.lines[0].canonical, "NSE:RELIANCE:2026-11-26:FUT");

    let index =
        value_book(document().as_bytes(), &[position("NIFTY26NOVFUT", "-65")]).expect("index");
    assert_eq!(index.lines[0].canonical, "NSE:NIFTY:2026-11-23:FUT");
    let stock = value_book(book().as_bytes(), &[position("RELIANCE", "5")]).expect("stock");
    assert_eq!(stock.lines[0].canonical, "NSE:RELIANCE:EQ");
    let call = quote_span_xml(
        book().as_bytes(),
        date(),
        &[position("NSE:NIFTY:2026-11-03:26000:CE", "-1")],
        &exposure(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect("call");
    assert_eq!(call.lines[0].canonical, "NSE:NIFTY:2026-11-03:26000:CE");

    let wrong = quote_span_xml(
        book().as_bytes(),
        date(),
        &[position("FUTIDX:RELIANCE:2026-11-26", "-1")],
        &exposure(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect_err("index prefix");
    assert!(wrong.contains("not in the contract file"), "{wrong}");

    let repeated = quote_span_xml(
        book().as_bytes(),
        date(),
        &[
            position("RELIANCE26N26FUT", "-1"),
            position("NSE:RELIANCE:2026-11-26:FUT", "1"),
        ],
        &exposure(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect_err("same contract");
    assert!(repeated.contains("same contract"), "{repeated}");
}

#[test]
fn a_bse_sensex_future_uses_the_iccl_risk_array_and_extreme_loss_percent() {
    let xml = r#"<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<exchange><exch>BSE</exch></exchange>
<phyPf><pfCode>BSX</pfCode><priceMeth>IDX</priceMeth>
<phy><pe>000000</pe><p>85106.81</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>0</a><a>0</a></ra></phy></phyPf>
<futPf><pfCode>BSXFUT</pfCode><priceMeth>IDX</priceMeth>
<fut><pe>20261029</pe><p>100</p><cvf>1</cvf><sc>1</sc><d>1</d>
<ra><r>1</r><a>-1</a><a>4</a></ra></fut>
<fut><pe>20261126</pe><p>100</p><cvf>1</cvf><sc>1</sc><d>1</d>
<ra><r>1</r><a>-1</a><a>4</a></ra></fut></futPf>
<futPf><pfCode>SX50FUT</pfCode><priceMeth>IDX</priceMeth>
<fut><pe>20261029</pe><p>100</p><cvf>1</cvf><sc>1</sc><d>1</d>
<ra><r>1</r><a>50</a><a>50</a></ra></fut></futPf>
<ccDef><cc>BSX</cc><name>BSE 30 SENSEX</name>
<riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<pfLink><pfCode>BSX</pfCode><pfType>PHY</pfType></pfLink>
<pfLink><pfCode>BSXFUT</pfCode><pfType>FUT</pfType></pfLink>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0</val></rate></tier></somTiers>
</ccDef>
<ccDef><cc>SX50</cc><name>S&amp;P BSE SENSEX 50</name>
<riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<pfLink><pfCode>SX50FUT</pfCode><pfType>FUT</pfType></pfLink>
</ccDef>
</spanFile>"#;
    let rates = ContractElmRates::parse_iccl_csv(
        b"864571,SENSEX26OCTFUT,IF,3.50,0.00,3.50\r\n864571,SENSEX26OCTFUT,IF,3.50,0.00,3.50\r\n",
    )
    .expect("elm");
    let quote = quote_span_xml(
        xml.as_bytes(),
        date(),
        &[position("BSE:SENSEX:2026-10-29:FUT", "-10")],
        &ExposureRates::none(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect("sensex");
    assert_eq!(quote.lines[0].canonical, "BSE:SENSEX:2026-10-29:FUT");
    assert_eq!(quote.lines[0].product, "SENSEX");
    assert_eq!(quote.scanning_risk, amount("10"));
    assert_eq!(quote.extreme_loss_margin, amount("35"));
    assert_eq!(quote.delivery_margin, amount("0"));
    assert_eq!(quote.premium, amount("0"));
    assert_eq!(quote.total_margin, amount("45"));
    assert_eq!(quote.lines[0].elm_percent, amount("3.50"));

    let missing = quote_span_xml(
        xml.as_bytes(),
        date(),
        &[position("BSE:SENSEX:2026-11-26:FUT", "-1")],
        &ExposureRates::none(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect_err("november");
    assert!(missing.contains("no future rate"), "{missing}");
    let index = parse_span_index(xml.as_bytes()).expect("index");
    let indexed = QuoteBuilder::on(date(), &[position("BSE:SENSEX:2026-10-29:FUT", "-10")])
        .contracts(&rates)
        .price_index(&index)
        .expect("indexed");
    assert_eq!(indexed, quote);
    let sha = "ab".repeat(32);
    let encoded = crate::span::index::write_index(&index, &sha).expect("encode");
    let loaded = crate::span::index::read_index(&encoded, &sha).expect("decode");
    let round = QuoteBuilder::on(date(), &[position("BSE:SENSEX:2026-10-29:FUT", "-10")])
        .contracts(&rates)
        .price_index(&loaded)
        .expect("round trip");
    assert_eq!(round, quote);
}

#[test]
fn a_span_index_reloads_the_same_nifty_quote() {
    let positions = [position("NIFTY26NOVFUT", "-65")];
    let direct = value_book(document().as_bytes(), &positions).expect("quote");
    let index = parse_span_index(document().as_bytes()).expect("index");
    let exposure = exposure();
    let indexed = QuoteBuilder::on(date(), &positions)
        .exposure(&exposure)
        .price_index(&index)
        .expect("indexed");
    assert_eq!(indexed, direct);
}

#[test]
fn iccl_extreme_loss_symbols_may_contain_a_hyphen_or_ampersand() {
    let rates = ContractElmRates::parse_iccl_csv(
        b"1,BAJAJ-AUTO26OCTFUT,IF,2.00,0.00,2.00\n\
          2,M&M26OCTFUT,IF,3.00,0.00,3.00\n\
          3,GVT&D26OCTFUT,SF,2.50,0.00,2.50\n\
          4,NAM-INDIA26OCTFUT,IF,1.00,0.00,1.00\n",
    )
    .expect("symbols");
    let october = date();
    assert_eq!(
        rates.monthly_future("BAJAJ-AUTO", october),
        Some(amount("2.00"))
    );
    assert_eq!(rates.monthly_future("M&M", october), Some(amount("3.00")));
    assert_eq!(rates.monthly_future("GVT&D", october), Some(amount("2.50")));
    assert_eq!(
        rates.monthly_future("NAM-INDIA", october),
        Some(amount("1.00"))
    );
    let options = ContractElmRates::parse_iccl_csv(
        b"1,SENSEX26OCT74000CE,IO,2.00,0.00,2.00\n\
          2,SENSEX26NOV75000PE,IO,2.00,0.00,2.00\n\
          3,SENSEX26N0573000CE,IO,3.00,0.00,3.00\n\
          4,SENSEX26NOV73000CE,IO,2.00,0.00,2.00\n\
          5,CANBK26OCT107.5CE,SO,4.00,0.00,4.00\n\
          6,BAJAJ-AUTO26OCT320.75PE,SO,5.00,0.00,5.00\n\
          7,NIFTY26N0320850PE,IO,2.00,0.00,2.00\n",
    )
    .expect("option symbols");
    let october_expiry = NaiveDate::from_ymd_opt(2026, 10, 29).expect("october");
    let november = NaiveDate::from_ymd_opt(2026, 11, 26).expect("november");
    let november_week = NaiveDate::from_ymd_opt(2026, 11, 5).expect("week");
    assert_eq!(
        options
            .option_rate(
                "SENSEX",
                ContractKind::Call,
                october_expiry,
                amount("74000")
            )
            .expect("october call"),
        amount("2.00")
    );
    assert_eq!(
        options
            .option_rate("SENSEX", ContractKind::Put, november, amount("75000"))
            .expect("november put"),
        amount("2.00")
    );
    assert_eq!(
        options
            .option_rate("SENSEX", ContractKind::Call, november_week, amount("73000"))
            .expect("weekly call"),
        amount("3.00")
    );
    assert_eq!(
        options
            .option_rate("SENSEX", ContractKind::Call, november, amount("73000"))
            .expect("monthly call"),
        amount("2.00")
    );
    assert_eq!(
        options
            .option_rate("CANBK", ContractKind::Call, october_expiry, amount("107.5"))
            .expect("decimal strike"),
        amount("4.00")
    );
    assert_eq!(
        options
            .option_rate(
                "BAJAJ-AUTO",
                ContractKind::Put,
                october_expiry,
                amount("320.75")
            )
            .expect("hyphen product"),
        amount("5.00")
    );
    assert_eq!(
        options
            .option_rate(
                "NIFTY",
                ContractKind::Put,
                NaiveDate::from_ymd_opt(2026, 11, 3).expect("weekly"),
                amount("20850")
            )
            .expect("shortest product"),
        amount("2.00")
    );
    let missing = options
        .option_rate("SENSEX", ContractKind::Call, november, amount("74000"))
        .expect_err("missing");
    assert!(missing.contains("no option rate"), "{missing}");
}

#[test]
fn a_bse_sensex_option_uses_the_iccl_option_percent_and_the_underlying_price() {
    let xml = r#"<spanFile>
<pointInTime><date>20261028</date></pointInTime>
<exchange><exch>BSE</exch></exchange>
<phyPf><pfCode>BSX</pfCode><priceMeth>IDX</priceMeth>
<phy><pe>000000</pe><p>85106.81</p><cvf>1</cvf><sc>1</sc>
<ra><r>1</r><a>0</a><a>0</a></ra></phy></phyPf>
<oopPf><pfCode>BSXOPT</pfCode><priceMeth>IDX</priceMeth>
<series><pe>20261029</pe><cvf>1</cvf><sc>1</sc>
<opt><o>C</o><k>74000</k><p>12.5</p>
<ra><r>1</r><a>-1</a><a>4</a></ra></opt>
</series>
<series><pe>20261105</pe><cvf>1</cvf><sc>1</sc>
<opt><o>C</o><k>73000</k><p>1</p>
<ra><r>1</r><a>-1</a><a>4</a></ra></opt>
</series>
</oopPf>
<ccDef><cc>BSX</cc><name>BSE 30 SENSEX</name>
<riskExponent>0</riskExponent><cmbMeth>NONE</cmbMeth>
<pfLink><pfCode>BSX</pfCode><pfType>PHY</pfType></pfLink>
<pfLink><pfCode>BSXOPT</pfCode><pfType>OOP</pfType></pfLink>
<somTiers><tier><tn>0</tn><rate><r>1</r><val>0</val></rate></tier></somTiers>
</ccDef>
</spanFile>"#;
    let rates = ContractElmRates::parse_iccl_csv(
        b"1,SENSEX26OCT74000CE,IO,2.00,0.00,2.00\n\
          2,SENSEX26N0573000CE,IO,3.00,0.00,3.00\n",
    )
    .expect("elm");
    let short_call = quote_span_xml(
        xml.as_bytes(),
        date(),
        &[position("BSE:SENSEX:2026-10-29:74000:CE", "-10")],
        &ExposureRates::none(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect("october call");
    assert_eq!(
        short_call.lines[0].canonical,
        "BSE:SENSEX:2026-10-29:74000:CE"
    );
    assert_eq!(short_call.scanning_risk, amount("10"));
    assert_eq!(short_call.extreme_loss_margin, amount("17021.36"));
    assert_eq!(short_call.lines[0].elm_percent, amount("2.00"));
    assert_eq!(short_call.premium, amount("-125"));
    assert_eq!(short_call.delivery_margin, amount("0"));

    let weekly = quote_span_xml(
        xml.as_bytes(),
        date(),
        &[position("BSE:SENSEX:2026-11-05:73000:CE", "-10")],
        &ExposureRates::none(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect("weekly");
    assert_eq!(weekly.extreme_loss_margin, amount("25532.04"));
    assert_eq!(weekly.lines[0].elm_percent, amount("3.00"));

    let long_call = quote_span_xml(
        xml.as_bytes(),
        date(),
        &[position("BSE:SENSEX:2026-10-29:74000:CE", "10")],
        &ExposureRates::none(),
        &rates,
        &CashMarginRates::default(),
    )
    .expect("long call");
    assert_eq!(long_call.extreme_loss_margin, amount("0"));
    assert_eq!(long_call.total_margin, amount("0"));

    let weekly_only = ContractElmRates::parse_iccl_csv(b"2,SENSEX26N0573000CE,IO,3.00,0.00,3.00\n")
        .expect("weekly only");
    let missing = quote_span_xml(
        xml.as_bytes(),
        date(),
        &[position("BSE:SENSEX:2026-10-29:74000:CE", "-10")],
        &ExposureRates::none(),
        &weekly_only,
        &CashMarginRates::default(),
    )
    .expect_err("missing strike");
    assert!(missing.contains("no option rate"), "{missing}");
}
