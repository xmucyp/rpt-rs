use super::*;

/// A cross-tab pivots the detail rows into a grid: region (rows) × quarter (cols), Sum(amt) cells,
/// rendered as native draw-ops.
#[test]
fn crosstab_renders_a_grid_from_data() {
    use rpt_pages::{DrawOp, ObjectKind};

    let saved = saved_data(
        &[
            ("t.region", FieldValueType::String),
            ("t.quarter", FieldValueType::String),
            ("t.amt", FieldValueType::Number),
        ],
        &[
            &["East", "Q1", "10"],
            &["West", "Q1", "20"],
            &["East", "Q2", "30"],
            &["West", "Q2", "40"],
        ],
    );
    let mut report = Report::default();
    report.data_definition.field_definitions = vec![crosstab_measure_def()];
    report.print_options.content_width = Twips(12240);
    report.print_options.content_height = Twips(15840);
    report.report_definition.areas = vec![
        area(
            AreaSectionKind::ReportHeader,
            vec![section(
                AreaSectionKind::ReportHeader,
                "RH",
                4200,
                vec![crosstab_object("CT1")],
            )],
        ),
        area(
            AreaSectionKind::Detail,
            vec![section(
                AreaSectionKind::Detail,
                "D",
                200,
                vec![text_object("row", "x", 0)],
            )],
        ),
    ];
    let src = SavedDataSource::new(&saved);
    let ds = build_dataset(&src, &report.data_definition);
    let formulas = rpt_data::compile_formulas(&report.data_definition);
    let doc = layout(&report, &ds, &formulas);

    // Collect the cross-tab-sourced draw-ops and their text.
    let texts: Vec<String> = doc
        .pages
        .iter()
        .flat_map(|p| &p.ops)
        .filter_map(|op| match op {
            DrawOp::Text(t)
                if t.source
                    .as_ref()
                    .is_some_and(|s| s.kind == ObjectKind::CrossTab) =>
            {
                Some(t.text.clone())
            }
            _ => None,
        })
        .collect();
    // Headers (East/West rows, Q1/Q2 cols) + the four cell sums (2-decimal formatted) must be present.
    for expected in [
        "East", "West", "Q1", "Q2", "10.00", "20.00", "30.00", "40.00",
    ] {
        assert!(
            texts.iter().any(|t| t == expected),
            "cross-tab grid missing {expected:?}: {texts:?}"
        );
    }
}

/// A cross-tab's decomposed cell objects (the decoder surfaces them flat in the section, inside the
/// cross-tab box) must not be drawn on top of the native grid; a real object outside the box still is.
#[test]
fn crosstab_decomposed_cell_objects_are_not_double_drawn() {
    let ct = crosstab_object("CT1"); // box: left 100, top 0, 8000 × 4000
    let inside = text_object("", "INSIDE_CELL", 500); // top-left (100, 500): within the cross-tab box
    let mut outside = text_object("Title", "OUTSIDE_TITLE", 0);
    outside.bounds.top = Twips(4500); // below the cross-tab box bottom (4000)

    let saved = saved_data(
        &[
            ("t.region", FieldValueType::String),
            ("t.quarter", FieldValueType::String),
            ("t.amt", FieldValueType::Number),
        ],
        &[&["East", "Q1", "10"]],
    );
    let mut report = Report::default();
    report.data_definition.field_definitions = vec![crosstab_measure_def()];
    report.print_options.content_width = Twips(12240);
    report.print_options.content_height = Twips(15840);
    report.report_definition.areas = vec![area(
        AreaSectionKind::ReportHeader,
        vec![section(
            AreaSectionKind::ReportHeader,
            "RH",
            5000,
            vec![ct, inside, outside],
        )],
    )];
    let ds = build_dataset(&SavedDataSource::new(&saved), &report.data_definition);
    let formulas = rpt_data::compile_formulas(&report.data_definition);
    let doc = layout(&report, &ds, &formulas);

    let texts: Vec<&str> = doc
        .pages
        .iter()
        .flat_map(|p| &p.ops)
        .filter_map(|op| match op {
            DrawOp::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !texts.contains(&"INSIDE_CELL"),
        "a cell object inside the cross-tab box must be suppressed (drawn by the grid): {texts:?}"
    );
    assert!(
        texts.contains(&"OUTSIDE_TITLE"),
        "an object outside the cross-tab box must still render: {texts:?}"
    );
}

// --- A cross-tab laid out from its template, as Crystal prints it (Canada - Cross Tab). ---

/// Where a template puts its grand totals: after the values (the designer's default, as in
/// Canada - Cross Tab) or before them (the `crosstab_*` parking reports).
#[derive(Clone, Copy, PartialEq)]
enum Totals {
    After,
    Before,
}

/// A template text object (the grand-total labels) at `(left, top)` within the cross-tab box.
fn template_text(text: &str, left: i32, top: i32, width: i32, height: i32) -> ReportObject {
    let mut o = text_object("", text, 0);
    o.bounds = Rect {
        left: Twips(100 + left),
        top: Twips(top),
        width: Twips(width),
        height: Twips(height),
    };
    o
}

/// A template field object (a dimension header or a summary) at `(left, top)` within the box.
fn template_field(
    source: &str,
    kind: FieldRefKind,
    left: i32,
    top: i32,
    w: i32,
    h: i32,
) -> ReportObject {
    let mut o = ReportObject::default();
    o.bounds = Rect {
        left: Twips(100 + left),
        top: Twips(top),
        width: Twips(w),
        height: Twips(h),
    };
    let mut f = FieldObject::default();
    f.data_source = source.to_string();
    f.ref_kind = kind;
    if kind == FieldRefKind::Summary {
        f.value_type = FieldValueType::Number;
    }
    o.kind = ReportObjectKind::Field(Box::new(f));
    o
}

/// Canada - Cross Tab's template, in its own geometry: a 1478-twip label column and 1128-twip value
/// columns, a 221-twip header row and 230-twip value rows, each cell inset by a 100-twip margin. The
/// cross-tab box (`crosstab_object`) sits at left 100, top 0.
fn template(totals: Totals) -> Vec<ReportObject> {
    let (data_x, total_x) = match totals {
        Totals::After => (1778, 3106),
        Totals::Before => (3106, 1778),
    };
    let (data_y, total_y) = match totals {
        Totals::After => (521, 951),
        Totals::Before => (951, 521),
    };
    let sum = |x, y| template_field("Sum ({t.amt})", FieldRefKind::Summary, x, y, 1128, 230);
    vec![
        template_field(
            "Column #1 Name",
            FieldRefKind::GroupName,
            data_x,
            100,
            1128,
            221,
        ),
        template_text("Total", total_x, 100, 1128, 221),
        template_field(
            "Row #1 Name",
            FieldRefKind::GroupName,
            100,
            data_y,
            1478,
            230,
        ),
        template_text("Total", 100, total_y, 1478, 230),
        sum(data_x, data_y),
        sum(total_x, data_y),
        sum(data_x, total_y),
        sum(total_x, total_y),
    ]
}

/// Rows deliberately out of order: West and Q2 come first.
fn template_rows() -> SavedData {
    saved_data(
        &[
            ("t.region", FieldValueType::String),
            ("t.quarter", FieldValueType::String),
            ("t.amt", FieldValueType::Number),
        ],
        &[
            &["West", "Q2", "40"],
            &["East", "Q1", "10"],
            &["West", "Q1", "20"],
            &["East", "Q2", "30"],
        ],
    )
}

/// A report whose header holds the cross-tab (`ct`) and its template, with a footer line after it.
fn template_report(ct: ReportObject, totals: Totals, content_width: i32) -> Report {
    let mut objects = vec![ct];
    objects.extend(template(totals));
    let mut report = Report::default();
    report.data_definition.field_definitions = vec![crosstab_measure_def()];
    report.print_options.content_width = Twips(content_width);
    report.print_options.content_height = Twips(15840);
    report.report_definition.areas = vec![
        area(
            AreaSectionKind::ReportHeader,
            vec![section(AreaSectionKind::ReportHeader, "RH", 1281, objects)],
        ),
        area(
            AreaSectionKind::ReportFooter,
            vec![section(
                AreaSectionKind::ReportFooter,
                "RF",
                300,
                vec![text_object("End", "END", 0)],
            )],
        ),
    ];
    report
}

fn crosstab_runs(doc: &rpt_pages::PagedDocument) -> Vec<&rpt_pages::TextRun> {
    doc.pages
        .iter()
        .flat_map(|p| &p.ops)
        .filter_map(|op| match op {
            DrawOp::Text(t)
                if t.source
                    .as_ref()
                    .is_some_and(|s| s.kind == rpt_pages::ObjectKind::CrossTab) =>
            {
                Some(t)
            }
            _ => None,
        })
        .collect()
}

/// `(left, top)` of the run printing `text`, the `nth` time it occurs in drawing order.
fn at(doc: &rpt_pages::PagedDocument, text: &str, nth: usize) -> (i32, i32) {
    let run = crosstab_runs(doc)
        .into_iter()
        .filter(|t| t.text == text)
        .nth(nth)
        .unwrap_or_else(|| panic!("no {text:?} #{nth}"));
    (run.bounds.left.0, run.bounds.top.0)
}

#[test]
fn a_crosstab_prints_its_template_band_by_band() {
    let doc = rendered(
        &template_report(crosstab_object("CT1"), Totals::After, 12240),
        &template_rows(),
    );
    // Columns: the 1678-twip label column, then a 1328-twip column per quarter, then Total; each
    // object sits at its own offset inside its cell, 100 twips from the cell's top-left.
    assert_eq!(at(&doc, "Q1", 0), (100 + 1778, 100));
    assert_eq!(at(&doc, "Q2", 0), (100 + 1778 + 1328, 100));
    assert_eq!(
        at(&doc, "Total", 0),
        (100 + 1778 + 2 * 1328, 100),
        "the Total column last"
    );
    // Rows: the 421-twip header row, then a 430-twip row per region, then Total.
    assert_eq!(at(&doc, "East", 0), (200, 521));
    assert_eq!(at(&doc, "West", 0), (200, 521 + 430));
    assert_eq!(
        at(&doc, "Total", 1),
        (200, 521 + 2 * 430),
        "the Total row last"
    );
    // Each value prints through its own summary object: East × Q1, West's row total, and the grand
    // total in the bottom-right cell.
    assert_eq!(at(&doc, "10.00", 0), (100 + 1778, 521));
    assert_eq!(at(&doc, "60.00", 0), (100 + 1778 + 2 * 1328, 521 + 430));
    assert_eq!(
        at(&doc, "100.00", 0),
        (100 + 1778 + 2 * 1328, 521 + 2 * 430)
    );
}

#[test]
fn a_template_with_its_totals_first_prints_them_first() {
    let doc = rendered(
        &template_report(crosstab_object("CT1"), Totals::Before, 12240),
        &template_rows(),
    );
    assert_eq!(
        at(&doc, "Total", 0),
        (100 + 1778, 100),
        "the Total column first"
    );
    assert_eq!(at(&doc, "Q1", 0), (100 + 1778 + 1328, 100));
    assert_eq!(at(&doc, "Total", 1), (200, 521), "the Total row first");
    assert_eq!(at(&doc, "East", 0), (200, 521 + 430));
}

#[test]
fn each_axis_prints_in_its_dimensions_sort_order() {
    let mut ct = crosstab_object("CT1");
    let ReportObjectKind::CrossTab(x) = &mut ct.kind else {
        unreachable!()
    };
    x.rows[0].direction = rpt_model::SortDirection::DescendingOrder;
    let doc = rendered(&template_report(ct, Totals::After, 12240), &template_rows());
    // Rows descend (West first); columns keep the designer's default, ascending (Q1 first), though
    // the data lists West and Q2 first.
    assert_eq!(at(&doc, "West", 0).1, 521);
    assert_eq!(at(&doc, "East", 0).1, 521 + 430);
    assert_eq!(at(&doc, "Q1", 0).0, 100 + 1778);
}

#[test]
fn a_crosstab_grows_its_section_to_the_rows_it_prints() {
    let doc = rendered(
        &template_report(crosstab_object("CT1"), Totals::After, 12240),
        &template_rows(),
    );
    // The grid is 421 + 3 × 430 = 1711 twips tall, against the section's designed 1281.
    let end = page_text_tops(&doc.pages[0], "END");
    assert_eq!(end, vec![1711]);
}

#[test]
fn columns_that_do_not_fit_across_the_page_are_not_printed() {
    // 100 + 1678 + 1328 = 3106 twips fit; the second quarter's column would end past 4000.
    let report = template_report(crosstab_object("CT1"), Totals::After, 4000);
    let ds = build_dataset(
        &SavedDataSource::new(&template_rows()),
        &report.data_definition,
    );
    let formulas = rpt_data::compile_formulas(&report.data_definition);
    let doc = layout(&report, &ds, &formulas);
    let texts: Vec<&str> = crosstab_runs(&doc)
        .iter()
        .map(|t| t.text.as_str())
        .collect();
    assert!(texts.contains(&"Q1") && !texts.contains(&"Q2"), "{texts:?}");
    assert!(
        doc.diagnostics
            .iter()
            .any(|d| d.message.contains("wider than the page")),
        "{:?}",
        doc.diagnostics
    );
}

#[test]
fn a_crosstab_cell_reserves_no_cell_for_its_negative_sign() {
    // A bracketed negative: a placed field keeps a trailing blank for the closing bracket, but
    // Crystal prints a cross-tab cell flush with its edge.
    let mut report = template_report(crosstab_object("CT1"), Totals::After, 12240);
    for o in &mut report.report_definition.areas[0].sections[0].objects {
        if let ReportObjectKind::Field(f) = &mut o.kind {
            if f.ref_kind == FieldRefKind::Summary {
                let mut fmt = rpt_model::FieldFormat::default();
                fmt.common.use_system_defaults = false;
                fmt.numeric.decimal_places = 2;
                fmt.numeric.negative = rpt_model::NegativeFormat::Bracketed;
                f.format = Some(fmt);
            }
        }
    }
    let doc = rendered(&report, &template_rows());
    assert!(crosstab_runs(&doc).iter().any(|t| t.text == "10.00"));
}

#[test]
fn the_empty_corner_cell_is_not_outlined() {
    let mut ct = crosstab_object("CT1");
    let ReportObjectKind::CrossTab(x) = &mut ct.kind else {
        unreachable!()
    };
    x.options.show_grid = true;
    let doc = rendered(&template_report(ct, Totals::After, 12240), &template_rows());
    let lines: Vec<_> = doc.pages[0]
        .ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Line(l) => Some(l),
            _ => None,
        })
        .collect();
    // The top rule starts at the first column header, the left rule at the first row label.
    let top = lines
        .iter()
        .find(|l| l.from.y.0 == 0 && l.to.y.0 == 0)
        .expect("top rule");
    assert_eq!(top.from.x.0, 100 + 1678);
    let left = lines
        .iter()
        .find(|l| l.from.x.0 == 100 && l.to.x.0 == 100)
        .expect("left rule");
    assert_eq!(left.from.y.0, 421);
}

#[test]
fn a_crosstab_over_no_rows_prints_a_zero_grand_total() {
    let empty = saved_data(
        &[
            ("t.region", FieldValueType::String),
            ("t.quarter", FieldValueType::String),
            ("t.amt", FieldValueType::Number),
        ],
        &[],
    );
    let doc = rendered(
        &template_report(crosstab_object("CT1"), Totals::After, 12240),
        &empty,
    );
    let texts: Vec<&str> = crosstab_runs(&doc)
        .iter()
        .map(|t| t.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["Total", "Total", "0.00"],
        "a Sum over nothing is zero, not blank"
    );
}

#[test]
fn stacked_summaries_each_print_their_own_measure() {
    // Two summaries stacked in each cell, as in the invoice-aging cross-tab: the upper one names no
    // summary the report defines, so it prints nothing, and the lower one still prints its own sum.
    let mut report = template_report(crosstab_object("CT1"), Totals::After, 12240);
    let objects = &mut report.report_definition.areas[0].sections[0].objects;
    objects.retain(
        |o| !matches!(&o.kind, ReportObjectKind::Field(f) if f.ref_kind == FieldRefKind::Summary),
    );
    for (x, y) in [(1778, 521), (3106, 521), (1778, 951), (3106, 951)] {
        objects.push(template_field(
            "Average ({t.nothing})",
            FieldRefKind::Summary,
            x,
            y,
            1128,
            115,
        ));
        objects.push(template_field(
            "Sum ({t.amt})",
            FieldRefKind::Summary,
            x,
            y + 115,
            1128,
            115,
        ));
    }
    let doc = rendered(&report, &template_rows());
    assert_eq!(
        at(&doc, "10.00", 0),
        (100 + 1778, 521 + 115),
        "East × Q1, in the lower slot"
    );
    assert!(crosstab_runs(&doc)
        .iter()
        .all(|t| t.bounds.top.0 != 521 || t.text == "East"));
}
