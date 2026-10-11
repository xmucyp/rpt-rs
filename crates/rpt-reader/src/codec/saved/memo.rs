//! The memo (variable-length / blob) values: each row's memo cells point into the per-batch heaps
//! decoded from `MemoValuesStream`, whose entries are `(u32 len)(payload)`.

use super::decrypt::{batch_iv, open_batch, OpenedBatch};
use super::schema::MEMO_CELL_SIZE;

/// The cipher's block length. A tail shorter than one block cannot carry a batch, since the zlib
/// gate reads the first block's keystream.
const CIPHER_BLOCK_LEN: usize = 16;

/// The length prefix each heap entry opens with, ahead of its text or binary payload.
const ENTRY_PREFIX_LEN: usize = 4;

/// Decode the per-batch memo heaps from `MemoValuesStream`: each batch is `zlib`-then-CFB with the
/// shared memo IV `(memo_cols, memo_cols*12, 12, 0)`, laid back-to-back. Returns each batch's inflated
/// heap bytes (the raw `(u32 len)(payload)` entry region, indexed by the descriptor-cell offsets). The
/// k-th heap aligns 1:1 with the k-th descriptor batch (both split by the same memo batch size).
pub(crate) fn decode_memo_heaps(memo_raw: &[u8], memo_cols: u32) -> Vec<Vec<u8>> {
    let iv = batch_iv(
        memo_cols,
        memo_cols.saturating_mul(MEMO_CELL_SIZE),
        MEMO_CELL_SIZE,
    );
    let mut heaps = Vec::new();
    let mut off = 0usize;
    while off + CIPHER_BLOCK_LEN <= memo_raw.len() {
        let Ok(OpenedBatch {
            inflated, consumed, ..
        }) = open_batch(&iv, &memo_raw[off..])
        else {
            break;
        };
        if consumed == 0 {
            break;
        }
        heaps.push(inflated);
        off += consumed;
    }
    heaps
}

/// Read one memo value out of a batch heap given a descriptor cell's `(offset, length)`. The offset
/// points at the entry's 4-byte length prefix; `length` is the value's UTF-16LE byte length including
/// the trailing NUL. Returns `None` for an empty value (length 0) or an out-of-bounds slice — an empty
/// persistent-memo is modelled as `None`.
pub(crate) fn read_memo_cell(heap: &[u8], offset: usize, length: usize) -> Option<String> {
    let data = read_memo_bytes(heap, offset, length)?;
    let s: String = char::decode_utf16(
        data.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0),
    )
    .map(|r| r.unwrap_or('\u{FFFD}'))
    .collect();
    (!s.is_empty()).then_some(s)
}

/// Read a descriptor's byte payload without interpreting it. Blob data may contain zero bytes or
/// an odd number of bytes, so applying the text decoder would both garble and truncate it.
pub(super) fn read_memo_bytes(heap: &[u8], offset: usize, length: usize) -> Option<&[u8]> {
    if length == 0 {
        return None;
    }
    let start = offset.checked_add(ENTRY_PREFIX_LEN)?;
    heap.get(start..start.checked_add(length)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::crypto::cfb_encrypt;

    #[test]
    fn binary_memo_keeps_zero_bytes_and_odd_lengths() {
        let payload = [0x89, 0x50, 0, 0, 0xff];
        let mut heap = (payload.len() as u32).to_le_bytes().to_vec();
        heap.extend_from_slice(&payload);
        assert_eq!(
            read_memo_bytes(&heap, 0, payload.len()),
            Some(payload.as_slice())
        );
        assert_eq!(read_memo_bytes(&heap, 0, payload.len() + 1), None);
        assert_eq!(read_memo_bytes(&heap, usize::MAX, 1), None);
        assert_eq!(read_memo_bytes(&heap, 0, usize::MAX), None);
        assert_eq!(read_memo_bytes(&heap, 0, 0), None);
    }

    #[test]
    fn decode_memo_heaps_walks_all_batches_and_cells_resolve() {
        // Two concatenated memo batches, each `zlib` then CFB with the shared memo IV
        // `(memo_cols, memo_cols*12, 12)`, then read a value out via a descriptor cell.
        let memo_cols = 5u32;
        let iv = batch_iv(memo_cols, memo_cols * 12, 12);
        let entry = |s: &str| {
            let mut v = (s.len() as u32 * 2 + 2).to_le_bytes().to_vec();
            for c in s.encode_utf16() {
                v.extend_from_slice(&c.to_le_bytes());
            }
            v.extend_from_slice(&[0, 0]); // trailing UTF-16 NUL
            v
        };
        let mut b1 = Vec::new();
        let a_off = b1.len();
        b1.extend(entry("alpha"));
        let beta_off = b1.len();
        b1.extend(entry("beta"));
        let mut b2 = Vec::new();
        b2.extend(entry("gamma"));
        let mut stream = cfb_encrypt(&iv, &miniz_oxide::deflate::compress_to_vec_zlib(&b1, 6));
        stream.extend(cfb_encrypt(
            &iv,
            &miniz_oxide::deflate::compress_to_vec_zlib(&b2, 6),
        ));
        let heaps = decode_memo_heaps(&stream, memo_cols);
        assert_eq!(heaps.len(), 2);
        // A descriptor cell's `(offset, length)` points at the entry (length prefix), length is the
        // value's UTF-16 byte count including the trailing NUL.
        assert_eq!(
            read_memo_cell(&heaps[0], a_off, 12).as_deref(),
            Some("alpha")
        );
        assert_eq!(
            read_memo_cell(&heaps[0], beta_off, 10).as_deref(),
            Some("beta")
        );
        assert_eq!(read_memo_cell(&heaps[1], 0, 12).as_deref(), Some("gamma"));
    }
}
