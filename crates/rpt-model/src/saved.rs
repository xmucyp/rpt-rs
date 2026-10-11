//! Saved (stored) data — the cached rows a report carries when saved with data.
//!
//! The stored records decoded from `SavedRecordsStream` + `MemoValuesStream`, as they sit in the
//! bytes — not the engine's result rowset (which projects, reorders, groups and formats them).

use super::enums::FieldValueType;

/// A report's stored saved data: the record count and the cached rows in record order.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SavedData {
    /// The stored record count.
    pub record_count: u32,
    /// The stored columns, in record order.
    pub columns: Vec<SavedColumn>,
    /// Row-major textual cells; `None` is null unless `binary_cells` supplies that position.
    pub rows: Vec<Vec<Option<String>>>,
    /// Sparse binary cells, kept separately so image bytes never pass through UTF-16 decoding.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub binary_cells: Vec<SavedBinaryCell>,
}

/// One non-null binary saved value. Its row/column indices are zero-based, like `SavedData::rows`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SavedBinaryCell {
    /// Zero-based record index.
    pub row: usize,
    /// Zero-based index into the saved columns.
    pub column: usize,
    /// The entire descriptor payload, including embedded zero bytes. JSON represents it as hex.
    #[cfg_attr(feature = "serde", serde(with = "crate::hex_bytes"))]
    pub data: Vec<u8>,
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn binary_saved_data_round_trips_without_changing_text_cells() {
        let saved = SavedData {
            record_count: 1,
            rows: vec![vec![Some("東京".into()), None]],
            binary_cells: vec![SavedBinaryCell {
                row: 0,
                column: 1,
                data: vec![0x89, 0, 0xff],
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&saved).expect("serialize");
        assert!(json.contains("\"data\":\"8900ff\""), "{json}");
        assert_eq!(
            serde_json::from_str::<SavedData>(&json).expect("deserialize"),
            saved
        );
        // Existing textual snapshots remain readable without the new optional binary collection.
        let old: SavedData =
            serde_json::from_str(r#"{"record_count":1,"columns":[],"rows":[["memo"]]}"#)
                .expect("old shape");
        assert!(old.binary_cells.is_empty());
        assert!(!serde_json::to_string(&old)
            .unwrap()
            .contains("binary_cells"));
    }
}

/// One stored saved-data column.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SavedColumn {
    /// The stored field name (e.g. `countries_all_iso.id`).
    pub name: String,
    /// The stored value type.
    pub value_type: FieldValueType,
}

/// A view of a report's saved-data batches: the decoded catalog schema,
/// the batch directory, and, per batch, the derived decrypt IV and whether it decrypts to a zlib
/// header. This is the data behind `rpt dump --saved` — it surfaces the encrypted-batch layer the
/// plain `dump` cannot reach, including batch classes the decoder does not model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SavedBatchInspection {
    /// The stored field catalog (record-layout order), with each field's inline byte offset.
    pub schema: Vec<SavedFieldInfo>,
    /// Number of variable-length (memo) columns — 0 means an all-inline record.
    pub memo_cols: u32,
    /// Raw byte length of `SavedRecordsStream` (the index + descriptor batches).
    pub srs_len: usize,
    /// Raw byte length of `MemoValuesStream` (the memo-value heaps); 0 when absent.
    pub memo_len: usize,
    /// One entry per batch in the `DataSourceManager` directory, in file order.
    pub batches: Vec<SavedBatchInfo>,
}

/// One stored field slot in the saved-data catalog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SavedFieldInfo {
    /// Byte offset of the field's slot within the fixed inline record.
    pub rec_offset: usize,
    /// The stored field name.
    pub name: String,
    /// Variable-length (memo/string) field — its value lives in `MemoValuesStream`.
    pub is_memo: bool,
}

/// The batch's role, keyed off its directory `item_size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SavedBatchKind {
    /// The fixed-width record index (`item_size` = the record width).
    Index,
    /// The memo descriptor (`item_size` = `memo_cols * 12`).
    Descriptor,
    /// A memo-value heap (`item_size` = 0, physically in `MemoValuesStream`).
    MemoValue,
}

/// One saved-data batch, its derived decrypt IV, and the outcome of trying it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SavedBatchInfo {
    /// The batch's role (index / descriptor / memo-value).
    pub kind: SavedBatchKind,
    /// Row count from the directory `0x6d` record.
    pub dir_count: u32,
    /// Fixed per-item byte width from the directory.
    pub dir_item_size: u32,
    /// 0-based sequence index of this batch within its kind (the IV's 4th word).
    pub seq: u32,
    /// The IV batch-size word (the row-capacity cap the decoder derives).
    pub iv_batch_size: u32,
    /// The IV item-count word.
    pub iv_item_count: u32,
    /// The IV persistent-item-size word.
    pub iv_item_size: u32,
    /// The 16 IV bytes decode would build for this batch.
    pub iv: Vec<u8>,
    /// Which raw stream this batch's ciphertext sits in: `false` = `SavedRecordsStream`,
    /// `true` = `MemoValuesStream`.
    pub in_memo_stream: bool,
    /// Byte offset of this batch's ciphertext within its stream.
    pub cursor: usize,
    /// True if decrypting block 0 with the derived IV yields a zlib header (`0x78 …`).
    pub decrypts_zlib: bool,
    /// Inflated byte length when the batch opens — under the derived IV, or stored plain
    /// (unencrypted zlib, as Crystal 8 writes it) — else `None`.
    pub inflated_len: Option<usize>,
    /// Bytes consumed when the batch opens (the next batch's offset), else `None`.
    pub consumed: Option<usize>,
    /// The first `item_size` record bytes of the inflated data region (empty on failure).
    pub first_record: Vec<u8>,
    /// The raw ciphertext head at `cursor` (for byte-level inspection when the IV fails).
    pub ct_head: Vec<u8>,
    /// The full `0x6d` directory entry for this batch — its field bytes, runs joined. Beyond
    /// `[count][item_size][stream_off][stream_len]` it carries, for a packed index batch, a
    /// `[u16 n_entries][n_entries × u32]` column table (`3 × string_columns` entries; every third
    /// value is the on-disk offset of the field after a compacted string). Surfaced so per-column
    /// packing is visible, not hidden.
    pub dir_entry: Vec<u8>,
}
