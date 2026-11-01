//! Parallel, order-preserving record formatting for the text writers.

use crate::error::PanResult;
use rayon::prelude::*;
use std::io::Write;

/// Number of S/L records formatted per parallel task.
pub const RECORDS_PER_CHUNK: usize = 1 << 14;

/// Formats `items` in parallel and writes the result in order.
///
/// `format` receives each item with its index in `items`. Only one window of
/// chunks (`per_chunk` items per thread) is held in memory at a time, so the
/// output text is never fully resident.
///
/// # Errors
/// Returns the first error of `format` (in item order) or of the writer.
pub fn write_records<T, F>(
    writer: &mut dyn Write,
    items: &[T],
    per_chunk: usize,
    format: F,
) -> PanResult<()>
where
    T: Sync,
    F: Fn(usize, &T, &mut Vec<u8>) -> PanResult<()> + Sync,
{
    let window = per_chunk * rayon::current_num_threads();
    for (w, window_items) in items.chunks(window).enumerate() {
        let window_start = w * window;
        let buffers: Vec<PanResult<Vec<u8>>> = window_items
            .par_chunks(per_chunk)
            .enumerate()
            .map(|(c, chunk)| {
                let chunk_start = window_start + c * per_chunk;
                let mut buf = Vec::new();
                for (i, item) in chunk.iter().enumerate() {
                    format(chunk_start + i, item, &mut buf)?;
                }
                Ok(buf)
            })
            .collect();
        for buf in buffers {
            writer.write_all(&buf?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PanGraphXError;
    use std::fmt::Write as _;

    /// Writes every item's index and value, across several chunks and windows.
    fn written(items: &[usize], per_chunk: usize, threads: usize) -> Vec<u8> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut out = Vec::new();
            write_records(&mut out, items, per_chunk, |index, &item, buf| {
                buf.extend_from_slice(format!("{index}:{item}\n").as_bytes());
                Ok(())
            })
            .unwrap();
            out
        })
    }

    #[test]
    fn write_records_passes_global_indices_in_order() {
        let items: Vec<usize> = (0..103).map(|i| i * 7).collect();
        let mut expected = String::new();
        for (i, v) in items.iter().enumerate() {
            writeln!(expected, "{i}:{v}").unwrap();
        }
        for (per_chunk, threads) in [(1, 1), (3, 2), (5, 4), (200, 3)] {
            assert_eq!(
                String::from_utf8(written(&items, per_chunk, threads)).unwrap(),
                expected,
                "per_chunk={per_chunk}, threads={threads}"
            );
        }
    }

    #[test]
    fn write_records_stops_at_first_error() {
        let items: Vec<usize> = (0..20).collect();
        let mut out = Vec::new();
        let result = write_records(&mut out, &items, 2, |index, _, buf| {
            if index >= 9 {
                return Err(PanGraphXError::Serialize(format!("item {index}")));
            }
            buf.push(b'x');
            Ok(())
        });
        assert!(matches!(result, Err(PanGraphXError::Serialize(m)) if m == "item 9"));
        assert!(out.len() <= 9, "nothing after the failing chunk is written");
    }
}
