//! Highlighting Expert rules (`0x00bf`), decoded onto the field object they follow. The two reports
//! in the corpus that carry any, each checked against Crystal's own render of it. Skips a fixture
//! that is absent.

use rpt_reader::model::{Color, HighlightOperator, ReportObjectKind};

fn highlights(name: &str, object: &str) -> Option<Vec<rpt_reader::model::HighlightRule>> {
    let path = rpt_test_support::fixture(format!("tests/fixtures/reports/benbrahim777/{name}.rpt"));
    if !path
        .try_exists()
        .expect("check highlighting fixture existence")
    {
        return None;
    }
    let rpt = rpt_reader::Rpt::open(&path)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    let report = rpt.report();
    let obj = report
        .objects()
        .find(|o| o.name == object)
        .unwrap_or_else(|| panic!("expected object {object:?} in {}", path.display()));
    match &obj.kind {
        ReportObjectKind::Field(f) => Some(f.highlights.clone()),
        _ => panic!(
            "expected object {object:?} to be a field in {}",
            path.display()
        ),
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color { a: 255, r, g, b }
}

#[test]
fn a_fields_rules_decode_in_the_order_they_are_tested() {
    let Some(rules) = highlights("Sales By Customer_grouped", "SumofOrderAmountCustomerName1")
    else {
        eprintln!("[skip] Sales By Customer_grouped.rpt absent");
        return;
    };
    // Crystal prints the $53.90 total red on grey, and totals over 100,000 in green.
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].operator, HighlightOperator::LessThan);
    assert_eq!(rules[0].threshold, "1000.00");
    assert_eq!(rules[0].font_color, Some(rgb(0xff, 0, 0)));
    assert_eq!(rules[0].background_color, Some(rgb(0xc0, 0xc0, 0xc0)));
    assert_eq!(rules[1].operator, HighlightOperator::GreaterThan);
    assert_eq!(rules[1].threshold, "100000.00");
    assert_eq!(rules[1].font_color, Some(rgb(0, 0x80, 0)));
    assert_eq!(
        rules[1].background_color, None,
        "flag 2 leaves the background alone"
    );
}

#[test]
fn objects_without_rules_carry_none() {
    let path = rpt_test_support::fixture("tests/fixtures/reports/benbrahim777/Orders5-150.rpt");
    if !path
        .try_exists()
        .expect("check highlighting fixture existence")
    {
        eprintln!("[skip] Orders5-150.rpt absent");
        return;
    }
    let rpt = rpt_reader::Rpt::open(&path)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    let report = rpt.report();
    assert!(report.objects().all(|o| match &o.kind {
        ReportObjectKind::Field(f) => f.highlights.is_empty(),
        _ => true,
    }));
}
