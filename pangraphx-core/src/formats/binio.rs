//! Helpers shared by the native binary readers (ODGI, `PackedGraph`, `HashGraph`):
//! exact reads that report a short input as a parse error naming the format,
//! and length checks before allocating from untrusted sizes.

use crate::error::{PanGraphXError, PanResult};
use std::io::{self, Read};

/// Largest buffer reserved up front for a length read from the input, so a
/// corrupt length fails with a parse error instead of a huge allocation.
pub const MAX_PREALLOC: usize = 1 << 20;

/// Maps an end of input to a parse error and keeps other I/O errors.
pub fn truncated(format: &str, error: io::Error) -> PanGraphXError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        PanGraphXError::Parse(format!("Truncated {format} file"))
    } else {
        PanGraphXError::Io(error)
    }
}

/// Reads exactly `N` bytes.
pub fn read_array<const N: usize, R: Read>(reader: &mut R, format: &str) -> PanResult<[u8; N]> {
    let mut buf = [0u8; N];
    reader
        .read_exact(&mut buf)
        .map_err(|e| truncated(format, e))?;
    Ok(buf)
}

/// Converts a length or index read from the input to `usize`.
pub fn to_usize(value: u64, format: &str, what: &str) -> PanResult<usize> {
    usize::try_from(value)
        .map_err(|_| PanGraphXError::Parse(format!("{format} {what} {value} is too large")))
}

/// Reads `len` bytes without trusting `len` for the initial allocation.
pub fn read_bytes<R: Read>(reader: &mut R, len: u64, format: &str) -> PanResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(to_usize(len, format, "length")?.min(MAX_PREALLOC));
    reader.by_ref().take(len).read_to_end(&mut buf)?;
    if buf.len() as u64 != len {
        return Err(PanGraphXError::Parse(format!("Truncated {format} file")));
    }
    Ok(buf)
}

/// Reads `count` little-endian `u64` words.
pub fn read_words_le<R: Read>(reader: &mut R, count: u64, format: &str) -> PanResult<Vec<u64>> {
    let count = to_usize(count, format, "word count")?;
    let mut words = Vec::with_capacity(count.min(MAX_PREALLOC));
    let mut block = [0u8; 8 * 1024];
    while words.len() < count {
        let n = (count - words.len()).min(block.len() / 8);
        let bytes = &mut block[..n * 8];
        reader.read_exact(bytes).map_err(|e| truncated(format, e))?;
        words.extend(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|w| u64::from_le_bytes(*w)),
        );
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn short_input_is_a_parse_error() {
        let err = read_array::<8, _>(&mut Cursor::new([1u8, 2]), "X").unwrap_err();
        assert!(matches!(err, PanGraphXError::Parse(m) if m == "Truncated X file"));
        assert!(read_bytes(&mut Cursor::new([1u8, 2]), 3, "X").is_err());
        assert!(read_words_le(&mut Cursor::new([0u8; 15]), 2, "X").is_err());
    }

    #[test]
    fn words_are_little_endian() {
        let mut bytes = Vec::new();
        for v in 0..2000_u64 {
            bytes.extend_from_slice(&(v * 0x0101).to_le_bytes());
        }
        let words = read_words_le(&mut Cursor::new(bytes), 2000, "X").unwrap();
        assert_eq!(words, (0..2000).map(|v| v * 0x0101).collect::<Vec<_>>());
    }
}
