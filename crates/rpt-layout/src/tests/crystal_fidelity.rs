//! Layout facts measured against pages the real Crystal Reports runtime rendered (the reference
//! renders of the benbrahim777 reports): how a stored font size becomes a drawn em, where a line's
//! baseline sits, which border sides draw, and how area-level and per-page section formatting
//! apply.

use super::*;
use rpt_model::{LineStyle, Subreport, SubreportObject};

fn runs<'a>(doc: &'a rpt_pages::PagedDocument, text: &str) -> Vec<(usize, &'a rpt_pages::TextRun)> {
    doc.pages
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.ops.iter().map(move |op| (i, op)))
        .filter_map(|(i, op)| match op {
            DrawOp::Text(t) if t.text == text => Some((i, t)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_report_font_is_drawn_at_the_em_its_gdi_cell_height_implies() {
    // Crystal hands the stored size to GDI as a cell height, so 10pt Arial draws at an 8.95pt em.
    let doc = rendered(&tiny_report(15840), &numeric_rows(1));
    let (_, run) = runs(&doc, "line")[0];
    assert_eq!(
        run.font.size_pt,
        10.0 * rpt_pages::gdi_cell_em_ratio("Arial")
    );
    assert!((run.font.size_pt - 8.951).abs() < 0.001);
}

#[test]
fn a_lines_baseline_sits_at_the_stored_sizes_ascent() {
    // The glyphs are drawn at the smaller em, but the baseline is the ascent of the stored size:
    // ApproxLayout's ascent is 0.8 em, so 0.8 × 10pt × 20 = 160 twips below the line top.
    let doc = rendered(&tiny_report(15840), &numeric_rows(1));
    let (_, run) = runs(&doc, "line")[0];
    assert_eq!(run.metrics.unwrap().ascent, Twips(160));
}

#[test]
fn the_cell_to_em_ratio_is_the_families_own() {
    use rpt_pages::gdi_cell_em_ratio as r;
    assert!((r("Arial") - 0.8951).abs() < 1e-4);
    assert!(
        (r("arial") - 0.8951).abs() < 1e-4,
        "matched without regard to case"
    );
    assert!((r("Verdana") - 0.8228).abs() < 1e-4);
    assert!((r("Times New Roman") - 0.9030).abs() < 1e-4);
    assert!((r("Courier New") - 0.8828).abs() < 1e-4);
    // An unlisted family takes the ratio of the Liberation face of its class that draws it.
    assert_eq!(r("Some Corporate Sans"), r("Arial"));
    assert_eq!(r("Some Corporate Serif"), r("Times New Roman"));
    assert_eq!(r("Some Mono"), r("Courier New"));
}

#[test]
fn a_page_header_suppressed_on_page_one_prints_from_page_two() {
    // `PageNumber = 1` as the page header's Section_Visibility keeps a running header off page 1.
    let mut report = tiny_report(1200);
    report.report_definition.areas[0].sections[0].condition_formulas =
        vec![("Section_Visibility".into(), "PageNumber = 1".into())];
    let doc = rendered(&report, &numeric_rows(12));
    assert!(doc.pages.len() >= 2, "enough rows for a second page");
    let pages: Vec<usize> = runs(&doc, "REPORT").into_iter().map(|(p, _)| p).collect();
    assert!(!pages.contains(&0), "no page header on page 1");
    assert!(pages.contains(&1), "the page header prints on page 2");
}

#[test]
fn an_areas_new_page_before_breaks_before_its_first_section() {
    // The flag is the area's, not the section's: Crystal applies it to the area as a whole.
    let mut report = tiny_report(15840);
    let mut footer = area(
        AreaSectionKind::ReportFooter,
        vec![section(
            AreaSectionKind::ReportFooter,
            "Footer",
            300,
            vec![text_object("Ftr", "THE END", 0)],
        )],
    );
    footer.format.base.new_page_before = true;
    report.report_definition.areas.push(footer);
    let doc = rendered(&report, &numeric_rows(2));
    assert_eq!(doc.pages.len(), 2);
    assert_eq!(runs(&doc, "THE END")[0].0, 1, "the footer starts page 2");
}

#[test]
fn an_area_suppressed_as_a_whole_hides_its_sections() {
    let mut report = tiny_report(15840);
    report.report_definition.areas[0].format.base.suppress = true;
    let doc = rendered(&report, &numeric_rows(1));
    assert!(runs(&doc, "REPORT").is_empty());
    assert_eq!(runs(&doc, "line").len(), 1, "other areas still print");
}

#[test]
fn a_bottom_border_draws_only_the_bottom_side() {
    let mut report = tiny_report(15840);
    let obj = &mut report.report_definition.areas[1].sections[0].objects[0];
    obj.border.bottom = LineStyle::SingleLine;
    let doc = rendered(&report, &numeric_rows(1));
    let ops = &doc.pages[0].ops;
    assert!(
        !ops.iter()
            .any(|op| matches!(op, DrawOp::Rect(r) if r.stroke.is_some())),
        "no four-sided frame"
    );
    let lines: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Line(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].from.y, lines[0].to.y, "horizontal");
    // The detail band starts below the 300-twip page header; the object is 240 twips tall.
    assert_eq!(
        lines[0].from.y,
        Twips(300 + 240),
        "along the object's bottom edge"
    );
}

#[test]
fn a_subreports_content_ignores_the_subreports_own_margins() {
    // Crystal draws a subreport's objects at the box plus their own offsets. A subreport saved with
    // quarter-inch margins must not shift its content up and left by them.
    let mut nested = Report::default();
    nested.print_options.margins = rpt_model::PageMargins {
        left: Twips(360),
        right: Twips(360),
        top: Twips(360),
        bottom: Twips(360),
    };
    nested.report_definition.areas = vec![area(
        AreaSectionKind::ReportHeader,
        vec![section(
            AreaSectionKind::ReportHeader,
            "SubRH",
            400,
            vec![text_object("Lbl", "SUBTEXT", 50)],
        )],
    )];
    let mut sub_obj = ReportObject::default();
    sub_obj.name = "SubObj".into();
    sub_obj.bounds = Rect {
        left: Twips(1000),
        top: Twips(500),
        width: Twips(3000),
        height: Twips(2000),
    };
    let mut so = SubreportObject::default();
    so.subreport_name = "Sub".into();
    sub_obj.kind = ReportObjectKind::Subreport(so);
    let mut main = Report::default();
    main.report_definition.areas = vec![area(
        AreaSectionKind::ReportHeader,
        vec![section(
            AreaSectionKind::ReportHeader,
            "MainRH",
            3000,
            vec![sub_obj],
        )],
    )];
    let mut sr = Subreport::default();
    sr.name = "Sub".into();
    sr.report = Box::new(nested);
    main.subreports = vec![sr];

    let doc = rendered(&main, &SavedData::default());
    let (_, run) = runs(&doc, "SUBTEXT")[0];
    assert_eq!((run.bounds.left.0, run.bounds.top.0), (1100, 550));
}

#[test]
fn new_page_before_does_not_break_before_the_report_body_begins() {
    // A group header set to New Page Before prints its first group under the report header on
    // page 1 (Crystal's SalesByCustomer-Grouped); later instances each start a page.
    let mut report = tiny_report(15840);
    report.report_definition.areas.insert(
        0,
        area(
            AreaSectionKind::ReportHeader,
            vec![section(
                AreaSectionKind::ReportHeader,
                "RH",
                300,
                vec![text_object("Title", "TITLE", 0)],
            )],
        ),
    );
    report.report_definition.areas[2]
        .format
        .base
        .new_page_before = true; // the Detail area
    let doc = rendered(&report, &numeric_rows(3));
    assert_eq!(
        doc.pages.len(),
        3,
        "a page per record, and no blank first page"
    );
    let first: Vec<&str> = doc.pages[0]
        .ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        first.contains(&"TITLE") && first.contains(&"line"),
        "page 1: {first:?}"
    );
}

#[test]
fn a_fields_drop_shadow_is_drawn() {
    let mut report = tiny_report(15840);
    let obj = &mut report.report_definition.areas[1].sections[0].objects[0];
    obj.border.top = LineStyle::SingleLine;
    obj.border.bottom = LineStyle::SingleLine;
    obj.border.left = LineStyle::SingleLine;
    obj.border.right = LineStyle::SingleLine;
    obj.border.has_drop_shadow = true;
    let doc = rendered(&report, &numeric_rows(1));
    let filled = doc.pages[0]
        .ops
        .iter()
        .filter(|op| matches!(op, DrawOp::Rect(r) if r.fill.is_some() && r.stroke.is_none()))
        .count();
    assert_eq!(filled, 2, "a bar below and a bar to the right");
}

/// A text stack with real metrics (not the approximate one), so the requested family's own ascent
/// decides the baseline.
#[derive(Debug)]
struct MeasuredLayout;

impl rpt_pages::TextLayout for MeasuredLayout {
    fn width_twips(&self, text: &str, font: &rpt_pages::FontSpec) -> f64 {
        text.chars().count() as f64 * f64::from(font.size_pt) * 10.0
    }
}

#[test]
fn the_baseline_takes_the_requested_familys_ascent() {
    // Verdana's ascent is 1.005 of its stored size, though a Liberation Sans face (0.905) draws it.
    let font = |family: &str| rpt_pages::FontSpec {
        family: family.to_string(),
        size_pt: 10.0 * rpt_pages::gdi_cell_em_ratio(family),
        ..rpt_pages::FontSpec::default()
    };
    let ascent = |family: &str| crate::text::crystal_ascent_twips(&MeasuredLayout, &font(family));
    assert!(
        (ascent("Verdana") - 201.07).abs() < 0.1,
        "1.00537 × 10pt × 20"
    );
    assert!(
        (ascent("Arial") - 181.05).abs() < 0.1,
        "0.90527 × 10pt × 20"
    );
    assert_eq!(rpt_pages::gdi_ascent_ratio("Some Corporate Sans"), None);
}
