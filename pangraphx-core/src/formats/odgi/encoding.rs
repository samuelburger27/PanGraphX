//! Little-endian integers and DYNAMIC `hacked_vector`s (fixed-width integers
//! packed into 64-bit words), as read and written by both sides.

use crate::error::{PanGraphXError, PanResult};
use std::io::{self, Read};

/// Largest buffer reserved up front for a length read from the input, so a
/// corrupt length fails with a parse error instead of a huge allocation.
pub(super) const MAX_PREALLOC: usize = 1 << 20;

pub(super) fn truncated(error: io::Error) -> PanGraphXError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        truncated_file()
    } else {
        PanGraphXError::Io(error)
    }
}

pub(super) fn read_u64<R: Read>(reader: &mut R) -> PanResult<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf).map_err(truncated)?;
    Ok(u64::from_le_bytes(buf))
}

pub(super) fn to_usize(value: u64, what: &str) -> PanResult<usize> {
    usize::try_from(value)
        .map_err(|_| PanGraphXError::Parse(format!("ODGI {what} {value} is too large")))
}

pub(super) fn read_bytes<R: Read>(reader: &mut R, len: u64) -> PanResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(to_usize(len, "length")?.min(MAX_PREALLOC));
    reader.by_ref().take(len).read_to_end(&mut buf)?;
    if buf.len() as u64 != len {
        return Err(truncated_file());
    }
    Ok(buf)
}

pub(super) fn truncated_file() -> PanGraphXError {
    PanGraphXError::Parse("Truncated ODGI file".to_string())
}

pub(super) fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap_or_default())
}

/// Reads the fields of node records from an in-memory buffer.
pub(super) struct Fields<'a> {
    pub(super) bytes: &'a [u8],
}

impl<'a> Fields<'a> {
    pub(super) fn take(&mut self, len: usize) -> PanResult<&'a [u8]> {
        let (head, tail) = self
            .bytes
            .split_at_checked(len)
            .ok_or_else(truncated_file)?;
        self.bytes = tail;
        Ok(head)
    }

    pub(super) fn u64(&mut self) -> PanResult<u64> {
        self.take(8).map(le_u64)
    }

    fn u8(&mut self) -> PanResult<u8> {
        self.take(1)
            .map(|byte| byte.first().copied().unwrap_or_default())
    }

    pub(super) fn len(&mut self, what: &str) -> PanResult<usize> {
        to_usize(self.u64()?, what)
    }

    /// Reads a DYNAMIC `hacked_vector` (fixed-width integers packed into
    /// 64-bit words) and unpacks its values into `values`.
    pub(super) fn packed(&mut self, values: &mut Vec<u64>) -> PanResult<()> {
        let word_count = self.len("word count")?;
        let words = self.take(word_count.checked_mul(8).ok_or_else(truncated_file)?)?;
        let _mask = self.u64()?;
        let len = self.len("vector length")?;
        let width = u32::from(self.u8()?);
        let _per_word = self.u8()?;
        values.clear();
        if len == 0 {
            return Ok(());
        }
        if width == 0 || width > 64 {
            return Err(PanGraphXError::Parse(format!(
                "Invalid ODGI packed vector width {width}"
            )));
        }
        let per_word = (64 / width) as usize;
        if word_count < len.div_ceil(per_word) {
            return Err(PanGraphXError::Parse(
                "ODGI packed vector is shorter than its length".to_string(),
            ));
        }
        let mask = u64::MAX >> (64 - width);
        // `len` is bounded by the words in the buffer, so this reservation is too.
        values.reserve(len);
        for chunk in words.as_chunks::<8>().0 {
            let mut word = u64::from_le_bytes(*chunk);
            for _ in 0..per_word.min(len - values.len()) {
                values.push(word & mask);
                // A shift by 64 (width 64) wraps to 0, but then `per_word` is 1.
                word = word.wrapping_shr(width);
            }
            if values.len() == len {
                break;
            }
        }
        Ok(())
    }
}

pub(super) fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Appends `values` as a `hacked_vector` of the smallest width that fits them.
#[allow(clippy::cast_possible_truncation)]
pub(super) fn push_packed(buf: &mut Vec<u8>, values: &[u64]) {
    let Some(&max) = values.iter().max() else {
        // A default-constructed (empty) vector.
        push_u64(buf, 0);
        push_u64(buf, 0);
        push_u64(buf, 0);
        buf.extend_from_slice(&[0, 0]);
        return;
    };
    let width = (64 - max.leading_zeros()).max(1) as usize;
    let per_word = 64 / width;
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1 << width) - 1
    };
    push_u64(buf, values.len().div_ceil(per_word) as u64);
    for chunk in values.chunks(per_word) {
        let word = chunk
            .iter()
            .enumerate()
            .fold(0, |word, (k, &value)| word | value << (k * width));
        push_u64(buf, word);
    }
    push_u64(buf, mask);
    push_u64(buf, values.len() as u64);
    buf.push(width as u8);
    buf.push(per_word as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_widths() {
        for values in [
            vec![0],
            vec![1, 0, 1],
            vec![u64::MAX, 3],
            (0..1000).collect(),
        ] {
            let mut buf = Vec::new();
            push_packed(&mut buf, &values);
            let mut read = Vec::new();
            Fields { bytes: &buf }.packed(&mut read).unwrap();
            assert_eq!(read, values);
        }
    }
}
