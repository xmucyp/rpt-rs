//! The saved-data rows as part of the semantic model.
//!
//! [`crate::codec::saved`] decodes text and binary cells using each column's value type;
//! this is where that type comes from. Resolving it means reading the report's own database field
//! catalog, so it is a model question, not a byte question, and it belongs above the codec layer.

use crate::codec::{decode_saved_rows, SavedFieldDesc};
use crate::coverage::SavedDataStatus;
use crate::model::{FieldKindData, FieldValueType, Report, SavedColumn, SavedData};
use crate::records::RecordStream;
use crate::StreamId;
use std::collections::HashMap;

/// Decode a report's stored saved data from its `SavedRecordsStream` (record index) and
/// `MemoValuesStream` (variable-length values), with the account of what became of it.
///
/// Returns the stored records — not the engine's result rowset, which projects/reorders/groups/
/// formats them. The rows are `None` whenever nothing decoded, which several very different
/// situations produce; the [`SavedDataStatus`] is what tells them apart, and it is always returned.
pub(crate) fn decode_saved_data(
    streams: &[RecordStream],
    report: &Report,
) -> (Option<SavedData>, SavedDataStatus) {
    let find = |pred: fn(&StreamId) -> bool| streams.iter().find(|s| pred(s.id()));
    let nothing = |status| (None, status);
    // The top-level `DataSourceManager` variant is inherently non-subdocument (nested streams stay
    // `StreamId::Other`), so no explicit Subdocument exclusion is needed. Its logical payload is
    // decoded once at stream-decode time.
    let dsm =
        find(|id| matches!(id, StreamId::DataSourceManager(_))).map(RecordStream::logical_bytes);
    let Some(dsm) = dsm.filter(|d| !d.is_empty()) else {
        return nothing(SavedDataStatus::NoCatalog);
    };

    // Decodable only when the field values are in an external MemoValuesStream. Reports with no memo
    // columns (all-inline) still decode: the memo stream may be absent.
    let memo_raw = find(|id| matches!(id, StreamId::MemoValuesStream(_)))
        .map(|s| s.encode())
        .unwrap_or_default();

    // The catalog is read before the row stream is looked for, so a report that carries neither is
    // reported as storing no fields rather than as missing a stream it never needed.
    let catalog = super::read_catalog(dsm);
    let schema = &catalog.fields;
    if schema.is_empty() {
        return nothing(SavedDataStatus::NoStoredFields);
    }
    let Some(srs_raw) =
        find(|id| matches!(id, StreamId::SavedRecordsStream(_))).map(|s| s.encode())
    else {
        return nothing(catalog.status_without_row_stream());
    };
    // Each stored column's value type: a text memo is `PersistentMemo`, a blob keeps `Blob`;
    // every other column takes
    // its declared type from the report's database field of the same qualified name (the inline
    // packed reader keys the on-disk field width on this — a `Number` is an 8-byte double, an
    // `Int32s` is 4 bytes, a `String` is a NUL-terminated UTF-16 run). Unmatched fields fall back to
    // `Int32s`.
    let field_types = saved_field_types(schema, report);
    // The stored rows: index batches (inline fields, packed or fixed) + memo-descriptor batches whose
    // cells point into the memo-value heaps (no delta reconstruction needed).
    let rowset = decode_saved_rows(&catalog, &srs_raw, &memo_raw, &field_types);
    if rowset.rows.is_empty() {
        return nothing(rowset.status);
    }
    let columns = schema
        .iter()
        .zip(field_types)
        .map(|(f, value_type)| SavedColumn {
            name: f.name.clone(),
            value_type,
        })
        .collect();
    (
        Some(SavedData {
            record_count: rowset.record_count,
            columns,
            rows: rowset.rows,
            binary_cells: rowset.binary_cells,
        }),
        rowset.status,
    )
}

/// Resolve each saved column's value type (schema order): a text memo is `PersistentMemo`; every
/// other column takes the declared type of the report database field with the same qualified name
/// (`Table.Field`, matched on both the table's stored name and its alias) or, in a report with no
/// tables, of the one database field definition with its name, defaulting to `Int32s`.
/// This is what tells the inline row reader a `Number` column is an 8-byte double vs an `Int32s`
/// 4-byte scalar vs a `String` — the DSM saved-field catalog itself carries no type code.
fn saved_field_types(schema: &[SavedFieldDesc], report: &Report) -> Vec<FieldValueType> {
    let mut by_name: HashMap<String, FieldValueType> = HashMap::new();
    for t in &report.database.tables {
        for f in &t.data_fields {
            by_name
                .entry(format!("{}.{}", t.name, f.name))
                .or_insert(f.value_type);
            if !t.alias.is_empty() {
                by_name
                    .entry(format!("{}.{}", t.alias, f.name))
                    .or_insert(f.value_type);
            }
        }
    }
    // A Crystal 8 report has no tables to read: its database field definitions name no table. A
    // stored column takes the type of the one such definition with its field name, if there is one.
    for f in schema {
        let Some((_, field)) = f.name.rsplit_once('.') else {
            continue;
        };
        if by_name.contains_key(&f.name) {
            continue;
        }
        let mut same_name = report
            .data_definition
            .field_definitions
            .iter()
            .filter(|d| matches!(d.kind, FieldKindData::Database(_)) && d.name == field);
        if let (Some(d), None) = (same_name.next(), same_name.next()) {
            by_name.insert(f.name.clone(), d.value_type);
        }
    }
    schema
        .iter()
        .map(|f| {
            let declared = by_name.get(&f.name).copied();
            if f.is_memo {
                // Memo describes the storage location, not the value's encoding: a PNG and a
                // UTF-16 string use the same descriptor layout but require different decoders.
                declared
                    .filter(|t| t.is_binary())
                    .unwrap_or(FieldValueType::PersistentMemo)
            } else {
                declared.unwrap_or(FieldValueType::Int32s)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DbField, FieldDef};

    fn stored(name: &str) -> SavedFieldDesc {
        SavedFieldDesc {
            rec_offset: 0,
            name: name.into(),
            is_memo: false,
        }
    }

    /// A Crystal 8 report decodes no tables, so its stored columns are typed from the database
    /// field definitions by name, when exactly one has that name.
    #[test]
    fn a_report_without_tables_types_its_columns_by_field_name() {
        let field = |name: &str, value_type| FieldDef {
            name: name.into(),
            value_type,
            kind: FieldKindData::Database(DbField::default()),
            ..FieldDef::default()
        };
        let mut report = Report::default();
        report.data_definition.field_definitions = vec![
            field("Months", FieldValueType::String),
            field("Cash", FieldValueType::Number),
            field("Twice", FieldValueType::Number),
            field("Twice", FieldValueType::String),
        ];
        let schema = [
            stored("sp_Sales;1.Months"),
            stored("sp_Sales;1.Cash"),
            stored("sp_Sales;1.Twice"),
            stored("sp_Sales;1.Missing"),
        ];
        assert_eq!(
            saved_field_types(&schema, &report),
            [
                FieldValueType::String,
                FieldValueType::Number,
                FieldValueType::Int32s,
                FieldValueType::Int32s
            ]
        );
    }
}
