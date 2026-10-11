//! Render a cross-tab object as a native Page-IR grid.
//!
//! A cross-tab pivots the data by one or more **row** dimensions (down the left) × **column**
//! dimensions (across the top), with a **measure** (an aggregate — e.g. `Sum of {amount}`) in each
//! cell. The decode exposes `CrossTabObject::{rows, columns}`; the layout engine computes the pivot
//! from the dataset and this module draws it as ordinary [`DrawOp`]s (cell rects + grid lines +
//! text), so it renders identically through every backend with no new dependency (the same approach
//! as the chart renderer).
//!
//! A cross-tab saved by Crystal stores a small **template** inside its box: a text or field object
//! per kind of cell, each with the size, font, alignment and number format that cell prints with
//! ([`Template`]). The grid is laid out from it as Crystal prints it, measured against Crystal's own
//! page for *Canada - Cross Tab*: the template's bands in their stored order (so its totals print
//! before or after the values as the template has them), a value band repeated per dimension value
//! in the dimension's sort order, every cell its template object's extent plus the cell margin, the
//! section grown to the grid's height, and only the columns that fit across the page. A cross-tab
//! with no template (a model built by hand) falls back to [`grid_ops`]'s default styling.
//!
//! Supports one row dimension × one column dimension × every measure (measures are drawn stacked in
//! each cell). Nested multi-level axes are not implemented, and a grid taller than the page does not
//! break across pages.

use crate::place::{fill_rect, hairline_stroke};
use rpt_formula::eval::Value;
use rpt_model::{Color, CrossTabGridOptions, Rect, ReportObject, ReportObjectKind, Twips};
use rpt_pages::{DrawOp, FontSpec, LineOp, ObjectKind, ObjectRef, Point, TextAlign, TextRun};

const HEADER_FILL: Color = Color {
    a: 255,
    r: 0xe8,
    g: 0xe8,
    b: 0xe8,
};
const GRID: Color = Color {
    a: 255,
    r: 0x99,
    g: 0x99,
    b: 0x99,
};
const TEXT: Color = Color {
    a: 255,
    r: 0x22,
    g: 0x22,
    b: 0x22,
};
/// Inset from a cell's edge to its text, in twips.
const CELL_PAD: i32 = 40;
/// Cross-tab cell font size, in points.
const CELL_FONT_PT: f32 = 8.0;

/// The computed pivot to draw: the corner label, the column headers (across the top), the row
/// headers (down the left), and `cells[r][c]` = the formatted measure for (row r, column c). The
/// grand totals are the measure re-aggregated across a whole axis: [`row_totals`](Self::row_totals)
/// one per row (drawn as the grand-total column), [`col_totals`](Self::col_totals) one per column
/// (drawn as the grand-total row), and [`grand_total`](Self::grand_total) over everything.
pub(crate) struct Grid {
    pub corner: String,
    pub col_headers: Vec<String>,
    pub row_headers: Vec<String>,
    pub cells: Vec<Vec<String>>,
    /// Per-row totals (the measure across every column), one per row header — the grand-total column.
    pub row_totals: Vec<String>,
    /// Per-column totals (the measure across every row), one per column header — the grand-total row.
    pub col_totals: Vec<String>,
    /// The grand grand-total, over every row and column.
    pub grand_total: String,
}

/// One logical grid cell: its text, alignment, weight, and background fill.
struct Cell {
    text: String,
    align: TextAlign,
    bold: bool,
    fill: Option<Color>,
}

/// Draw the cross-tab `grid` inside `rect` (twips) as cell rects, grid lines, and text. The header
/// row/column are shaded; data cells are right-aligned (measures are numeric). The decoded grid
/// [`options`](CrossTabGridOptions) drive the display: grid lines are emitted only when
/// [`show_grid`](CrossTabGridOptions::show_grid) is set.
///
/// This is the fallback for a cross-tab with no [`Template`] to lay it out by. The grand totals sit
/// first — the grand-total column right after the row labels, the grand-total row right below the
/// column labels. Their colors are supplied on
/// the axis RAS names them by, which is the opposite axis to where they draw: `RowGrandTotalColor`
/// ([`row_grand_total_color`](CrossTabGridOptions::row_grand_total_color)) fills the grand-total
/// *column* (the per-row totals), `ColumnGrandTotalColor` the grand-total *row*. Each grand total is
/// dropped when its suppress flag is set — `EnableSuppressRowGrandTotals` drops the column,
/// `EnableSuppressColumnGrandTotals` the row (same axis convention as the colors).
pub(crate) fn grid_ops(
    rect: Rect,
    grid: &Grid,
    opts: &CrossTabGridOptions,
    section_name: &str,
    obj_name: &str,
    base_instance: u32,
) -> Vec<DrawOp> {
    if grid.col_headers.is_empty() || grid.row_headers.is_empty() {
        return Vec::new();
    }
    // Which grand-total bands are drawn, and the fixed indices they occupy (right after the labels).
    let has_gt_col = !grid.row_totals.is_empty() && !opts.suppress_row_grand_totals;
    let has_gt_row = !grid.col_totals.is_empty() && !opts.suppress_column_grand_totals;
    let gt_col = 1usize;
    let gt_row = 1usize;
    let data_c0 = 1 + has_gt_col as usize;
    let data_r0 = 1 + has_gt_row as usize;
    let ncols = data_c0 + grid.col_headers.len();
    let nrows = data_r0 + grid.row_headers.len();

    // Build the logical grid: text/alignment/weight per cell, then the background fills.
    let blank = || Cell {
        text: String::new(),
        align: TextAlign::Left,
        bold: false,
        fill: None,
    };
    let mut m: Vec<Vec<Cell>> = (0..nrows)
        .map(|_| (0..ncols).map(|_| blank()).collect())
        .collect();
    let put =
        |m: &mut Vec<Vec<Cell>>, r: usize, c: usize, s: &str, align: TextAlign, bold: bool| {
            m[r][c].text = s.to_string();
            m[r][c].align = align;
            m[r][c].bold = bold;
        };

    // Header labels: corner, the grand-total column header, and the column headers.
    put(&mut m, 0, 0, &grid.corner, TextAlign::Left, true);
    if has_gt_col {
        put(&mut m, 0, gt_col, "Total", TextAlign::Center, true);
    }
    for (c, h) in grid.col_headers.iter().enumerate() {
        put(&mut m, 0, data_c0 + c, h, TextAlign::Center, true);
    }
    // Grand-total row (per-column totals) and its label + the corner grand total.
    if has_gt_row {
        put(&mut m, gt_row, 0, "Total", TextAlign::Left, true);
        if has_gt_col {
            put(
                &mut m,
                gt_row,
                gt_col,
                &grid.grand_total,
                TextAlign::Right,
                false,
            );
        }
        for (c, v) in grid.col_totals.iter().enumerate() {
            put(&mut m, gt_row, data_c0 + c, v, TextAlign::Right, false);
        }
    }
    // Row labels, the grand-total column (per-row totals), and the data cells.
    for (r, rh) in grid.row_headers.iter().enumerate() {
        let rr = data_r0 + r;
        put(&mut m, rr, 0, rh, TextAlign::Left, true);
        if has_gt_col {
            let v = grid.row_totals.get(r).map(String::as_str).unwrap_or("");
            put(&mut m, rr, gt_col, v, TextAlign::Right, false);
        }
        for c in 0..grid.col_headers.len() {
            let v = grid
                .cells
                .get(r)
                .and_then(|row| row.get(c))
                .map(String::as_str)
                .unwrap_or("");
            put(&mut m, rr, data_c0 + c, v, TextAlign::Right, false);
        }
    }

    // Fills: header shading, then the grand-total colors (which override the header grey on their
    // own band). The grand-total colors are cross-wired to the drawn axis — see the fn doc.
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let mut fill = (r == 0 || c == 0).then_some(HEADER_FILL);
            if has_gt_col && c == gt_col {
                fill = opts.row_grand_total_color.or(fill);
            }
            if has_gt_row && r == gt_row {
                fill = opts.column_grand_total_color.or(fill);
            }
            cell.fill = fill;
        }
    }

    let src = || Some(ObjectRef::new(section_name, ObjectKind::CrossTab).named(obj_name));
    let (rl, rt, rw, rh) = (rect.left.0, rect.top.0, rect.width.0, rect.height.0);
    let col_w = (rw / ncols as i32).max(1);
    let row_h = (rh / nrows as i32).max(1);
    let cell_x = |c: usize| rl + c as i32 * col_w;
    let cell_y = |r: usize| rt + r as i32 * row_h;
    let mut ops: Vec<DrawOp> = Vec::new();

    // Background fills.
    for (r, row) in m.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if let Some(color) = cell.fill {
                ops.push(fill_rect(
                    Rect {
                        left: Twips(cell_x(c)),
                        top: Twips(cell_y(r)),
                        width: Twips(col_w),
                        height: Twips(row_h),
                    },
                    color,
                    src(),
                ));
            }
        }
    }

    // Grid lines (horizontal + vertical), enclosing the whole grid. Emitted only when the decoded
    // `EnableShowGrid` option is set; a grid-off cross-tab draws its shaded headers and cell text
    // with no interior or bounding rules.
    if opts.show_grid {
        let line = |ops: &mut Vec<DrawOp>, x1: i32, y1: i32, x2: i32, y2: i32| {
            ops.push(DrawOp::Line(LineOp {
                from: Point {
                    x: Twips(x1),
                    y: Twips(y1),
                },
                to: Point {
                    x: Twips(x2),
                    y: Twips(y2),
                },
                stroke: hairline_stroke(GRID),
                source: src(),
            }));
        };
        let right = cell_x(ncols);
        let bottom = cell_y(nrows);
        for r in 0..=nrows {
            line(&mut ops, rl, cell_y(r), right, cell_y(r));
        }
        for c in 0..=ncols {
            line(&mut ops, cell_x(c), rt, cell_x(c), bottom);
        }
    }

    // Text: each cell's content, aligned, clipped to its box (a small inset). A cell may stack
    // several measures (`\n`-joined), each drawn on its own line, splitting the cell height evenly.
    // Each drawn line gets its own per-placement instance id so a backend keeps it a separate
    // positioned element (like the engine) rather than merging a cell's stacked measures into one
    // multi-line paragraph.
    let mut inst = base_instance;
    for (r, row) in m.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if cell.text.is_empty() {
                continue;
            }
            let lines: Vec<&str> = cell.text.split('\n').collect();
            let inner_h = row_h - 2 * CELL_PAD;
            let line_h = (inner_h / lines.len() as i32).max(1);
            for (li, line) in lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                let source = Some(
                    ObjectRef::new(section_name, ObjectKind::CrossTab)
                        .named(obj_name)
                        .with_instance(inst),
                );
                inst += 1;
                ops.push(DrawOp::Text(TextRun {
                    bounds: Rect {
                        left: Twips(cell_x(c) + CELL_PAD),
                        top: Twips(cell_y(r) + CELL_PAD + li as i32 * line_h),
                        width: Twips(col_w - 2 * CELL_PAD),
                        height: Twips(line_h),
                    },
                    text: line.to_string(),
                    font: FontSpec {
                        family: "Arial".into(),
                        size_pt: CELL_FONT_PT,
                        bold: cell.bold,
                        ..Default::default()
                    },
                    color: TEXT,
                    align: cell.align,
                    rotation: 0.0,
                    metrics: None,
                    character_spacing: Twips(0),
                    source,
                }));
            }
        }
    }

    ops
}

impl crate::Formatter<'_> {
    /// Render a cross-tab object as a native grid: pivot the dataset by the cross-tab's
    /// first row dimension × first column dimension, aggregating every measure (stacked) into each
    /// cell. A cross-tab with a template is laid out from it ([`Self::emit_crosstab_template`]);
    /// without one it falls back to the default grid, and to a placeholder + diagnostic when the
    /// pivot structure or data is missing.
    pub(crate) fn emit_crosstab(
        &mut self,
        ct: &rpt_model::CrossTabObject,
        rect: Rect,
        section_name: &str,
        obj: &rpt_model::ReportObject,
    ) {
        // A cross-tab with a template prints as Crystal lays it out; one without (a model built by
        // hand) falls back to the default grid below.
        let objects = self.section_objects(section_name);
        let available = self.printable_width() - (self.col_offset + obj.bounds.left.0);
        if let Some(lay) = self.crosstab_template_layout(ct, obj, objects, available) {
            self.emit_crosstab_template(&lay, ct, rect, section_name, obj);
            return;
        }
        let row_field = ct.rows.iter().find(|d| !d.field_ref.is_empty());
        let col_field = ct.columns.iter().find(|d| !d.field_ref.is_empty());
        let (Some(row_field), Some(col_field)) = (row_field, col_field) else {
            crate::push_diag(
                &self.diagnostics,
                rpt_pages::Diagnostic::warn(
                    rpt_pages::DiagnosticKind::UnsupportedObject,
                    "cross-tab is missing a row/column dimension or measure; rendered as a placeholder",
                )
                .with_source(&obj.name),
            );
            self.placeholder_box(rect, section_name, obj, ObjectKind::CrossTab);
            return;
        };
        // The measures are the report's summary definitions, attributed to the cross-tab: the file
        // records no link from a summary back to the grid that aggregates it.
        let measures = rpt_model::crosstab_measures(self.report);
        if measures.is_empty() {
            crate::push_diag(
                &self.diagnostics,
                rpt_pages::Diagnostic::warn(
                    rpt_pages::DiagnosticKind::UnsupportedObject,
                    "cross-tab is missing a row/column dimension or measure; rendered as a placeholder",
                )
                .with_source(&obj.name),
            );
            self.placeholder_box(rect, section_name, obj, ObjectKind::CrossTab);
            return;
        }
        // Every measure is drawn stacked in each data cell (the engine renders all of them).
        let grid = crate::aggregate::crosstab_pivot(
            self.dataset,
            self.formulas,
            &self.locale,
            row_field,
            col_field,
            &measures,
            Some((&self.diagnostics, &obj.name)),
        );
        if grid.col_headers.is_empty() || grid.row_headers.is_empty() {
            crate::push_diag(
                &self.diagnostics,
                rpt_pages::Diagnostic::warn(
                    rpt_pages::DiagnosticKind::UnsupportedObject,
                    "cross-tab has no data to pivot; rendered as a placeholder",
                )
                .with_source(&obj.name),
            );
            self.placeholder_box(rect, section_name, obj, ObjectKind::CrossTab);
            return;
        }
        // Reserve a block of per-placement instance ids for the grid's text lines so each is a
        // separate positioned element downstream (unique across the report, no cross-object merge).
        let base_instance = self.next_instance_id;
        let ops = grid_ops(
            rect,
            &grid,
            &ct.options,
            section_name,
            &obj.name,
            base_instance,
        );
        self.next_instance_id += ops.iter().filter(|o| matches!(o, DrawOp::Text(_))).count() as u32;
        for op in ops {
            self.push_op(op);
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// The cross-tab as Crystal lays it out: from the cell objects stored inside its box.
// ---------------------------------------------------------------------------------------------------

/// What a template band lays out: the labels (the row-label column, the column-header row), the
/// values of a dimension (repeated once per value), or that dimension's grand total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Label,
    Data,
    Total,
}

/// One column or row of a cross-tab's template: its role, and where its objects start and how far
/// they reach along the axis, in report twips. The cell it becomes is that reach plus the cell margin
/// on both sides.
#[derive(Debug, Clone, Copy)]
struct Band {
    role: Role,
    start: i32,
    size: i32,
}

/// A cross-tab's design-time template. Crystal stores a cross-tab as a small grid of ordinary text
/// and field objects inside its box: the column-header field and its grand-total label across the
/// top, the row-header field and its grand-total label down the left, and one summary field per
/// (row band, column band) — several stacked when the grid has several measures. Each object carries
/// the size, font, alignment and number format its cells print with, and the order of the bands is
/// the order Crystal prints them in: a grand-total band before its values puts the totals first.
pub(crate) struct Template<'r> {
    /// Inset from a cell's edge to its objects: the template's first object sits this far into the
    /// box (100 twips with cell margins on, 0 with them off).
    margin: i32,
    cols: Vec<Band>,
    rows: Vec<Band>,
    /// Every visible template object, with the (column band, row band) it belongs to.
    cells: Vec<(usize, usize, &'r ReportObject)>,
}

impl<'r> Template<'r> {
    /// Read the template of `ct` from the objects of its section, or `None` when its box holds no
    /// template with one value band on each axis (the grid is then drawn from defaults).
    pub(crate) fn find(ct: &ReportObject, objects: &'r [ReportObject]) -> Option<Template<'r>> {
        let members: Vec<&ReportObject> = objects
            .iter()
            .filter(|o| {
                matches!(
                    o.kind,
                    ReportObjectKind::Field(_) | ReportObjectKind::Text(_)
                ) && o.bounds.width.0 > 0
                    && o.bounds.height.0 > 0
                    && crate::place::rect_contains_top_left(&ct.bounds, &o.bounds)
            })
            .collect();
        let left0 = members.iter().map(|o| o.bounds.left.0).min()?;
        let top0 = members.iter().map(|o| o.bounds.top.0).min()?;
        let margin = (left0 - ct.bounds.left.0).max(0);
        let role = |o: &ReportObject| match o.kind {
            ReportObjectKind::Text(_) => Role::Total,
            _ => Role::Data,
        };
        // The row-label column and the column-header row are the template's first column and row.
        // Down the label column, each object below the header row starts a row band; across the header
        // row, each object right of the label column starts a column band. A band's role is its label:
        // a field names the dimension's values, a text the grand total.
        let mut cols = vec![Band {
            role: Role::Label,
            start: left0,
            size: 0,
        }];
        let mut rows = vec![Band {
            role: Role::Label,
            start: top0,
            size: 0,
        }];
        for o in &members {
            let (l, t, w, h) = (
                o.bounds.left.0,
                o.bounds.top.0,
                o.bounds.width.0,
                o.bounds.height.0,
            );
            if l == left0 {
                cols[0].size = cols[0].size.max(w);
                if t != top0 {
                    merge_band(&mut rows, t, h, role(o));
                }
            }
            if t == top0 {
                rows[0].size = rows[0].size.max(h);
                if l != left0 {
                    merge_band(&mut cols, l, w, role(o));
                }
            }
        }
        cols.sort_by_key(|b| b.start);
        rows.sort_by_key(|b| b.start);
        let one_data = |bands: &[Band]| bands.iter().filter(|b| b.role == Role::Data).count() == 1;
        if !one_data(&cols) || !one_data(&rows) {
            return None;
        }
        let band_of = |bands: &[Band], at: i32| bands.iter().rposition(|b| b.start <= at);
        let cells = members
            .iter()
            .filter_map(|o| {
                Some((
                    band_of(&cols, o.bounds.left.0)?,
                    band_of(&rows, o.bounds.top.0)?,
                    *o,
                ))
            })
            .collect();
        Some(Template {
            margin,
            cols,
            rows,
            cells,
        })
    }

    /// The template objects of one (column band, row band) cell, top to bottom.
    fn objects_at(&self, col: usize, row: usize) -> Vec<&'r ReportObject> {
        let mut v: Vec<&ReportObject> = self
            .cells
            .iter()
            .filter(|(c, r, _)| (*c, *r) == (col, row))
            .map(|(_, _, o)| *o)
            .collect();
        v.sort_by_key(|o| (o.bounds.top.0, o.bounds.left.0));
        v
    }

    /// The measures the grid aggregates, and for each stacked summary of a cell (top to bottom, as
    /// in the data cell) which of them it prints: read from the summary the object names
    /// (`Sum ({Orders.Order Amount})`), matched against the report's summaries. A summary that names
    /// none of them prints nothing, without shifting the ones below it.
    fn measures(
        &self,
        report: &rpt_model::Report,
    ) -> (Vec<rpt_model::CrossTabMeasure>, Vec<Option<usize>>) {
        let (dc, dr) = (self.data_col(), self.data_row());
        let known = rpt_model::crosstab_measures(report);
        let mut measures: Vec<rpt_model::CrossTabMeasure> = Vec::new();
        let slots = self
            .objects_at(dc, dr)
            .into_iter()
            .filter_map(|o| match &o.kind {
                ReportObjectKind::Field(f) => Some(f),
                _ => None,
            })
            .map(|f| {
                let m = known.iter().find(|m| names_measure(&f.data_source, m))?;
                Some(match measures.iter().position(|x| x == m) {
                    Some(i) => i,
                    None => {
                        measures.push(m.clone());
                        measures.len() - 1
                    }
                })
            })
            .collect();
        (measures, slots)
    }

    fn data_col(&self) -> usize {
        self.cols
            .iter()
            .position(|b| b.role == Role::Data)
            .unwrap_or(0)
    }

    fn data_row(&self) -> usize {
        self.rows
            .iter()
            .position(|b| b.role == Role::Data)
            .unwrap_or(0)
    }
}

/// Add a band starting at `start`, or widen the one already there.
fn merge_band(bands: &mut Vec<Band>, start: i32, size: i32, role: Role) {
    match bands.iter_mut().find(|b| b.start == start) {
        Some(b) => b.size = b.size.max(size),
        None => bands.push(Band { role, start, size }),
    }
}

/// Whether a summary object's data source (`DistinctCount ({invoice.id})`, or with a second
/// group-scope argument) names `m`.
fn names_measure(data_source: &str, m: &rpt_model::CrossTabMeasure) -> bool {
    let ds = data_source.trim();
    ds.strip_prefix(m.operation.full_name())
        .is_some_and(|rest| rest.trim_start().starts_with('('))
        && ds.contains(&format!("{{{}}}", m.field))
}

/// One printed column or row of the grid: a template band, which of the band's values it prints,
/// and where it sits from the grid's origin.
#[derive(Debug, Clone, Copy)]
struct Track {
    band: usize,
    /// For a value band, the index of the dimension value; `0` otherwise.
    key: usize,
    pos: i32,
    size: i32,
}

/// A cross-tab laid out from its template: the computed pivot, and the columns and rows it prints.
pub(crate) struct TemplateLayout<'r> {
    template: Template<'r>,
    pivot: crate::aggregate::Pivot,
    row_dim: &'r rpt_model::CrossTabDimension,
    col_dim: &'r rpt_model::CrossTabDimension,
    cols: Vec<Track>,
    rows: Vec<Track>,
    /// For each stacked summary of a cell, the measure it prints (see [`Template::measures`]).
    slots: Vec<Option<usize>>,
    /// How many value columns did not fit across the page.
    dropped_cols: usize,
}

impl TemplateLayout<'_> {
    /// The grid's printed height, which grows its section.
    pub(crate) fn height(&self) -> i32 {
        self.rows.last().map_or(0, |r| r.pos + r.size)
    }
}

/// Lay out a band's tracks along one axis: a label or total band once, a value band once per value.
/// A grand-total band is dropped when `suppress_total` is set.
fn tracks(bands: &[Band], margin: i32, keys: usize, suppress_total: bool) -> Vec<Track> {
    let mut out = Vec::new();
    let mut pos = 0;
    for (i, b) in bands.iter().enumerate() {
        let n = match b.role {
            Role::Label => 1,
            Role::Data => keys,
            Role::Total if suppress_total => 0,
            Role::Total => 1,
        };
        let size = b.size + 2 * margin;
        for key in 0..n {
            out.push(Track {
                band: i,
                key,
                pos,
                size,
            });
            pos += size;
        }
    }
    out
}

impl<'a> crate::Formatter<'a> {
    /// The objects of the report's section named `name` (empty when there is none).
    pub(crate) fn section_objects(&self, name: &str) -> &'a [ReportObject] {
        self.report
            .report_definition
            .areas
            .iter()
            .flat_map(|a| &a.sections)
            .find(|s| s.name == name)
            .map_or(&[], |s| s.objects.as_slice())
    }

    /// The page's printable width: the paper less its left and right margins.
    pub(crate) fn printable_width(&self) -> i32 {
        self.page_size.width.0 - self.origin.x.0 - self.report.print_options.margins.right.0
    }

    /// How tall the cross-tab `obj` prints, laid out from its template among `objects`, or `None`
    /// when it has none (its stored box height then stands).
    pub(crate) fn crosstab_height(
        &self,
        obj: &ReportObject,
        objects: &[ReportObject],
    ) -> Option<i32> {
        let ReportObjectKind::CrossTab(ct) = &obj.kind else {
            return None;
        };
        let available = self.printable_width() - (self.col_offset + obj.bounds.left.0);
        self.crosstab_template_layout(ct, obj, objects, available)
            .map(|lay| lay.height())
    }

    /// Lay out `ct` from the template objects of its section, or `None` when it has no usable
    /// template. `available_width` is how much of the page's printable width lies right of the
    /// cross-tab: Crystal prints only the columns that fit whole.
    pub(crate) fn crosstab_template_layout<'r>(
        &self,
        ct: &'r rpt_model::CrossTabObject,
        obj: &ReportObject,
        objects: &'r [ReportObject],
        available_width: i32,
    ) -> Option<TemplateLayout<'r>>
    where
        'a: 'r,
    {
        let row_dim = ct.rows.iter().find(|d| !d.field_ref.is_empty())?;
        let col_dim = ct.columns.iter().find(|d| !d.field_ref.is_empty())?;
        let template = Template::find(obj, objects)?;
        let (measures, slots) = template.measures(self.report);
        if measures.is_empty() {
            return None;
        }
        let pivot = crate::aggregate::pivot_values(
            self.dataset,
            self.formulas,
            row_dim,
            col_dim,
            &measures,
            Some((&self.diagnostics, &obj.name)),
        );
        let opts = &ct.options;
        // The axes are cross-wired as RAS names them: suppressing the row grand totals drops the
        // grand-total column (the per-row totals), and vice versa.
        let mut cols = tracks(
            &template.cols,
            template.margin,
            pivot.col_keys.len(),
            opts.suppress_row_grand_totals,
        );
        let rows = tracks(
            &template.rows,
            template.margin,
            pivot.row_keys.len(),
            opts.suppress_column_grand_totals,
        );
        let fits = cols
            .iter()
            .take_while(|c| c.pos + c.size <= available_width)
            .count()
            .max(2.min(cols.len()));
        let dropped_cols = cols[fits..]
            .iter()
            .filter(|c| template.cols[c.band].role == Role::Data)
            .count();
        cols.truncate(fits);
        Some(TemplateLayout {
            template,
            pivot,
            row_dim,
            col_dim,
            cols,
            rows,
            slots,
            dropped_cols,
        })
    }

    /// Draw a cross-tab laid out from its template at `rect`'s top-left: the cell fills, each cell's
    /// objects as text in their own font, alignment and format, and the grid lines (every cell but
    /// the empty top-left corner is outlined).
    pub(crate) fn emit_crosstab_template(
        &mut self,
        lay: &TemplateLayout,
        ct: &rpt_model::CrossTabObject,
        rect: Rect,
        section_name: &str,
        obj: &ReportObject,
    ) {
        if lay.dropped_cols > 0 {
            crate::push_diag(
                &self.diagnostics,
                rpt_pages::Diagnostic::warn(
                    rpt_pages::DiagnosticKind::UnsupportedObject,
                    format!(
                        "cross-tab is wider than the page; {} column(s) that do not fit are not printed",
                        lay.dropped_cols
                    ),
                )
                .with_source(&obj.name),
            );
        }
        let t = &lay.template;
        let (x0, y0) = (rect.left.0, rect.top.0);
        let src = |inst: Option<u32>| {
            let r = ObjectRef::new(section_name, ObjectKind::CrossTab).named(&obj.name);
            Some(match inst {
                Some(i) => r.with_instance(i),
                None => r,
            })
        };
        let opts = &ct.options;
        // Cell fills: a template object's own background fills its cell; the grand-total colors fill
        // their band (cross-wired as RAS names them, see `grid_ops`).
        for row in &lay.rows {
            for col in &lay.cols {
                let (cb, rb) = (t.cols[col.band], t.rows[row.band]);
                let fill = t
                    .objects_at(col.band, row.band)
                    .iter()
                    .find_map(|o| o.border.background_color)
                    .or(match (cb.role, rb.role) {
                        (Role::Total, Role::Data | Role::Total) => opts.row_grand_total_color,
                        (Role::Data, Role::Total) => opts.column_grand_total_color,
                        _ => None,
                    });
                if let Some(color) = fill {
                    self.push_op(fill_rect(
                        Rect {
                            left: Twips(x0 + col.pos),
                            top: Twips(y0 + row.pos),
                            width: Twips(col.size),
                            height: Twips(row.size),
                        },
                        color,
                        src(None),
                    ));
                }
            }
        }
        // Cell text.
        for row in &lay.rows {
            for col in &lay.cols {
                let objects = t.objects_at(col.band, row.band);
                let mut summary = 0;
                for o in objects {
                    let text = match &o.kind {
                        ReportObjectKind::Text(tx) => tx.text.clone(),
                        ReportObjectKind::Field(f) => {
                            let k = summary;
                            summary += 1;
                            match self.crosstab_cell_text(lay, f, col, row, k) {
                                Some(s) => s,
                                None => continue,
                            }
                        }
                        _ => continue,
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let (cb, rb) = (t.cols[col.band], t.rows[row.band]);
                    let bounds = Rect {
                        left: Twips(x0 + col.pos + t.margin + (o.bounds.left.0 - cb.start)),
                        top: Twips(y0 + row.pos + t.margin + (o.bounds.top.0 - rb.start)),
                        width: o.bounds.width,
                        height: o.bounds.height,
                    };
                    let inst = self.next_instance_id;
                    self.next_instance_id += 1;
                    let run = self.crosstab_run(o, text, bounds, src(Some(inst)));
                    self.push_op(run);
                }
            }
        }
        if !opts.show_grid {
            return;
        }
        let (Some(last_col), Some(last_row)) = (lay.cols.last(), lay.rows.last()) else {
            return;
        };
        let right = x0 + last_col.pos + last_col.size;
        let bottom = y0 + last_row.pos + last_row.size;
        // The corner cell (header row × label column) is not outlined: the top rule starts at the
        // first column header, the left rule at the first row label.
        let corner_w = lay.cols.first().map_or(0, |c| c.size);
        let corner_h = lay.rows.first().map_or(0, |r| r.size);
        let line = |x1: i32, y1: i32, x2: i32, y2: i32| {
            DrawOp::Line(LineOp {
                from: Point {
                    x: Twips(x1),
                    y: Twips(y1),
                },
                to: Point {
                    x: Twips(x2),
                    y: Twips(y2),
                },
                stroke: hairline_stroke(GRID_LINE),
                source: src(None),
            })
        };
        let mut ops = Vec::new();
        for (i, r) in lay.rows.iter().enumerate() {
            let from = if i == 0 { x0 + corner_w } else { x0 };
            ops.push(line(from, y0 + r.pos, right, y0 + r.pos));
        }
        ops.push(line(x0, bottom, right, bottom));
        for (i, c) in lay.cols.iter().enumerate() {
            let from = if i == 0 { y0 + corner_h } else { y0 };
            ops.push(line(x0 + c.pos, from, x0 + c.pos, bottom));
        }
        ops.push(line(right, y0, right, bottom));
        for op in ops {
            self.push_op(op);
        }
    }

    /// The text a template field prints in one grid cell: a dimension value in a header, or the
    /// measure of the cell's `k`-th stacked summary, formatted through the field's own format.
    fn crosstab_cell_text(
        &self,
        lay: &TemplateLayout,
        f: &rpt_model::FieldObject,
        col: &Track,
        row: &Track,
        k: usize,
    ) -> Option<String> {
        let t = &lay.template;
        let p = &lay.pivot;
        let (cr, rr) = (t.cols[col.band].role, t.rows[row.band].role);
        let header = |v: &Value, dim: &rpt_model::CrossTabDimension| match dim.period {
            Some(_) => crate::aggregate::format_period_label(
                v,
                &self.locale,
                crate::aggregate::LabelPeriod::from_group(dim.period),
            ),
            None => self.crosstab_format(f, v),
        };
        let k = lay.slots.get(k).copied().flatten();
        let value = match (rr, cr) {
            (Role::Label, Role::Data) => {
                return Some(header(p.col_keys.get(col.key)?, lay.col_dim))
            }
            (Role::Data, Role::Label) => {
                return Some(header(p.row_keys.get(row.key)?, lay.row_dim))
            }
            (Role::Data, Role::Data) => p.cells.get(row.key)?.get(col.key)?.get(k?)?,
            (Role::Data, Role::Total) => p.row_totals.get(row.key)?.get(k?)?,
            (Role::Total, Role::Data) => p.col_totals.get(col.key)?.get(k?)?,
            (Role::Total, Role::Total) => p.grand.get(k?)?,
            _ => return None,
        };
        Some(self.crosstab_format(f, value))
    }

    /// A cell value formatted through its template field. Unlike a placed field, a cross-tab cell
    /// reserves no character cell for its negative form: Crystal prints a bracketed-negative amount
    /// flush with the cell's edge (Canada - Cross Tab), where the same format on a group-header field
    /// keeps a blank cell for the closing bracket (Top5USAsubCanada).
    fn crosstab_format(&self, f: &rpt_model::FieldObject, v: &Value) -> String {
        let vt = rpt_model::field_object_value_type(self.report, f);
        let mut spec = crate::format::field_format_spec(f.format.as_ref(), vt, &self.locale);
        match &mut spec {
            rpt_format_value::FormatSpec::Number(nf) => nf.reserve_sign = false,
            rpt_format_value::FormatSpec::Currency(cf) => cf.number.reserve_sign = false,
            _ => {}
        }
        crate::format::render_value(v, &spec, &self.locale)
    }

    /// One line of cell text, drawn as a text object of the template's prints: its font at the GDI
    /// em, its color, its resolved alignment and Crystal's baseline.
    fn crosstab_run(
        &self,
        o: &ReportObject,
        text: String,
        bounds: Rect,
        source: Option<ObjectRef>,
    ) -> DrawOp {
        let font_color = match &o.kind {
            ReportObjectKind::Field(f) => &f.font_color,
            ReportObjectKind::Text(t) => &t.font_color,
            _ => unreachable!("template cells are text and field objects"),
        };
        let font = crate::font_of(&font_color.font);
        let advance = Twips(self.text_layout.width_twips(&text, &font) as i32);
        DrawOp::Text(TextRun {
            bounds,
            text,
            align: crate::resolved_align(self.report, o, rpt_model::ReadingOrder::LeftToRight),
            color: font_color.color,
            rotation: 0.0,
            metrics: Some(rpt_pages::TextMetrics {
                advance,
                ascent: Twips(crate::text::crystal_ascent_twips(self.text_layout, &font) as i32),
                line_height: Twips(self.text_layout.line_height_twips(&font) as i32),
            }),
            character_spacing: Twips(0),
            font,
            source,
        })
    }
}

/// Crystal's grid rules are black.
const GRID_LINE: Color = Color {
    a: 255,
    r: 0,
    g: 0,
    b: 0,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Grid options with the grid lines enabled (the visible default the render tests exercise).
    fn opts_show_grid() -> CrossTabGridOptions {
        CrossTabGridOptions {
            show_grid: true,
            ..Default::default()
        }
    }

    fn sample_grid() -> Grid {
        Grid {
            corner: "Region".into(),
            col_headers: vec!["Q1".into(), "Q2".into()],
            row_headers: vec!["East".into(), "West".into()],
            cells: vec![
                vec!["10".into(), "20".into()],
                vec!["30".into(), "40".into()],
            ],
            // Unique markers so a total is never confused with a body cell in the text assertions.
            row_totals: vec!["RTa".into(), "RTb".into()],
            col_totals: vec!["CTa".into(), "CTb".into()],
            grand_total: "GG".into(),
        }
    }

    fn rect() -> Rect {
        Rect {
            left: Twips(0),
            top: Twips(0),
            width: Twips(8000),
            height: Twips(8000),
        }
    }

    const RED: Color = Color {
        a: 255,
        r: 255,
        g: 0,
        b: 0,
    };
    const BLUE: Color = Color {
        a: 255,
        r: 0,
        g: 0,
        b: 255,
    };

    /// The text strings drawn, in op order.
    fn texts(ops: &[DrawOp]) -> Vec<String> {
        ops.iter()
            .filter_map(|o| match o {
                DrawOp::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect()
    }

    /// The solid-fill rects drawn, as `(color, left, top)`.
    fn fills(ops: &[DrawOp]) -> Vec<(Color, i32, i32)> {
        ops.iter()
            .filter_map(|o| match o {
                DrawOp::Rect(r) => match r.fill {
                    Some(rpt_pages::Fill::Solid(c)) => Some((c, r.bounds.left.0, r.bounds.top.0)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn empty_grid_yields_no_ops() {
        let g = Grid {
            corner: "x".into(),
            col_headers: vec![],
            row_headers: vec![],
            cells: vec![],
            row_totals: vec![],
            col_totals: vec![],
            grand_total: String::new(),
        };
        assert!(grid_ops(rect(), &g, &opts_show_grid(), "S", "CT", 0).is_empty());
    }

    #[test]
    fn draws_headers_cells_and_grand_totals() {
        let ops = grid_ops(rect(), &sample_grid(), &opts_show_grid(), "RH", "CT1", 0);
        let t = texts(&ops);
        // 4×4 grid: corner + "Total" col header + 2 col headers; the grand-total row (label + grand
        // + 2 col totals); two data rows (label + row total + 2 cells) = 16 text cells.
        assert_eq!(t.len(), 16);
        // The grand totals are present.
        assert!(t.contains(&"GG".to_string()), "grand grand-total drawn");
        assert!(t.contains(&"CTa".to_string()), "a column total drawn");
        assert!(t.contains(&"RTa".to_string()), "a row total drawn");
        assert_eq!(
            t.iter().filter(|s| *s == "Total").count(),
            2,
            "two Total labels"
        );
        // A 4×4 grid → 5 horizontal + 5 vertical lines.
        let lines = ops.iter().filter(|o| matches!(o, DrawOp::Line(_))).count();
        assert_eq!(lines, 10);
    }

    #[test]
    fn row_grand_total_color_fills_the_grand_total_column() {
        // RAS `RowGrandTotalColor` is cross-wired: it colors the grand-total COLUMN (per-row
        // totals), which sits at grid column index 1 (right after the row labels).
        let opts = CrossTabGridOptions {
            show_grid: true,
            row_grand_total_color: Some(RED),
            ..Default::default()
        };
        let ops = grid_ops(rect(), &sample_grid(), &opts, "RH", "CT1", 0);
        let col_w = 8000 / 4; // ncols == 4
        let red: Vec<_> = fills(&ops)
            .into_iter()
            .filter(|(c, ..)| *c == RED)
            .collect();
        // Every cell of the grand-total column is red — all four rows, none elsewhere.
        assert_eq!(red.len(), 4, "grand-total column fully colored");
        assert!(
            red.iter().all(|(_, left, _)| *left == col_w),
            "red fills land only on the grand-total column (index 1)"
        );
    }

    #[test]
    fn column_grand_total_color_fills_the_grand_total_row() {
        // RAS `ColumnGrandTotalColor` colors the grand-total ROW (per-column totals) at row index 1.
        let opts = CrossTabGridOptions {
            show_grid: true,
            column_grand_total_color: Some(BLUE),
            ..Default::default()
        };
        let ops = grid_ops(rect(), &sample_grid(), &opts, "RH", "CT1", 0);
        let row_h = 8000 / 4; // nrows == 4
        let blue: Vec<_> = fills(&ops)
            .into_iter()
            .filter(|(c, ..)| *c == BLUE)
            .collect();
        assert_eq!(blue.len(), 4, "grand-total row fully colored");
        assert!(
            blue.iter().all(|(_, _, top)| *top == row_h),
            "blue fills land only on the grand-total row (index 1)"
        );
    }

    #[test]
    fn suppress_row_grand_totals_drops_the_column() {
        // `EnableSuppressRowGrandTotals` drops the grand-total COLUMN (the per-row totals).
        let opts = CrossTabGridOptions {
            show_grid: true,
            suppress_row_grand_totals: true,
            ..Default::default()
        };
        let ops = grid_ops(rect(), &sample_grid(), &opts, "RH", "CT1", 0);
        let t = texts(&ops);
        // The per-row totals and the corner grand total are gone; the per-column totals remain.
        assert!(!t.contains(&"RTa".to_string()) && !t.contains(&"RTb".to_string()));
        assert!(!t.contains(&"GG".to_string()), "corner grand total dropped");
        assert!(t.contains(&"CTa".to_string()), "column totals kept");
    }

    #[test]
    fn suppress_column_grand_totals_drops_the_row() {
        // `EnableSuppressColumnGrandTotals` drops the grand-total ROW (the per-column totals).
        let opts = CrossTabGridOptions {
            show_grid: true,
            suppress_column_grand_totals: true,
            ..Default::default()
        };
        let ops = grid_ops(rect(), &sample_grid(), &opts, "RH", "CT1", 0);
        let t = texts(&ops);
        assert!(!t.contains(&"CTa".to_string()) && !t.contains(&"CTb".to_string()));
        assert!(!t.contains(&"GG".to_string()), "corner grand total dropped");
        assert!(t.contains(&"RTa".to_string()), "row totals kept");
    }

    #[test]
    fn show_grid_off_suppresses_grid_lines() {
        let opts = CrossTabGridOptions::default();
        let ops = grid_ops(rect(), &sample_grid(), &opts, "RH", "CT1", 0);
        assert_eq!(
            ops.iter().filter(|o| matches!(o, DrawOp::Line(_))).count(),
            0,
            "grid lines suppressed"
        );
        // The headers, cells, and grand totals are still drawn (grid-off hides only the rules).
        assert_eq!(texts(&ops).len(), 16, "cells still drawn");
    }

    #[test]
    fn show_grid_toggle_only_affects_lines() {
        let on = grid_ops(rect(), &sample_grid(), &opts_show_grid(), "RH", "CT1", 0);
        let off = grid_ops(
            rect(),
            &sample_grid(),
            &CrossTabGridOptions::default(),
            "RH",
            "CT1",
            0,
        );
        let non_line =
            |ops: &[DrawOp]| ops.iter().filter(|o| !matches!(o, DrawOp::Line(_))).count();
        // Toggling the grid changes only the line ops; every other op (shading + text) is identical.
        assert_eq!(non_line(&on), non_line(&off));
    }
}
