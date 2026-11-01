//! Splitting the input into blocks of whole lines, and lines into fields.

use crate::error::PanResult;
use crate::formats::common::text::lossy;
use log::debug;
use memchr::{memchr, memrchr};
use std::io::{ErrorKind, Read};

/// Reads the input in blocks that end on a line boundary.
///
/// A block holds at least `block_size` bytes unless the input ends first; a
/// line longer than that grows the block until the line is complete.
pub(super) struct BlockReader<'r, R> {
    reader: &'r mut R,
    buf: Vec<u8>,
    /// Length of the block returned last; the rest of `buf` is an incomplete line.
    consumed: usize,
    block_size: usize,
    eof: bool,
}

impl<'r, R: Read> BlockReader<'r, R> {
    pub(super) fn new(reader: &'r mut R, block_size: usize) -> Self {
        Self {
            reader,
            buf: Vec::new(),
            consumed: 0,
            block_size: block_size.max(1),
            eof: false,
        }
    }

    /// Returns the next block of whole lines, or `None` at the end of the input.
    /// The last line of the input may lack its newline.
    pub(super) fn next_block(&mut self) -> PanResult<Option<&[u8]>> {
        self.buf.drain(..self.consumed);
        self.consumed = 0;
        let mut target = self.block_size;
        loop {
            self.fill_to(target)?;
            if let Some(last_newline) = memrchr(b'\n', &self.buf) {
                self.consumed = last_newline + 1;
                return Ok(Some(&self.buf[..self.consumed]));
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                self.consumed = self.buf.len();
                return Ok(Some(&self.buf));
            }
            // A single line is longer than the block: keep reading.
            target = self.buf.len().saturating_mul(2);
        }
    }

    fn fill_to(&mut self, target: usize) -> PanResult<()> {
        while !self.eof && self.buf.len() < target {
            let len = self.buf.len();
            self.buf.resize(target, 0);
            let read = self.reader.read(&mut self.buf[len..]);
            match read {
                Ok(0) => {
                    self.buf.truncate(len);
                    self.eof = true;
                }
                Ok(n) => self.buf.truncate(len + n),
                Err(e) if e.kind() == ErrorKind::Interrupted => self.buf.truncate(len),
                Err(e) => {
                    self.buf.truncate(len);
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }
}

/// Iterator over the parts of a byte string separated by `sep`.
///
/// An empty input yields one empty part, like [`slice::split`], but the
/// separator search uses `memchr`.
pub(super) struct Split<'a> {
    rest: Option<&'a [u8]>,
    sep: u8,
}

impl<'a> Split<'a> {
    pub(super) const fn new(bytes: &'a [u8], sep: u8) -> Self {
        Self {
            rest: Some(bytes),
            sep,
        }
    }
}

impl<'a> Iterator for Split<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        let rest = self.rest?;
        if let Some(i) = memchr(self.sep, rest) {
            self.rest = Some(&rest[i + 1..]);
            Some(&rest[..i])
        } else {
            self.rest = None;
            Some(rest)
        }
    }
}

/// Iterator over the lines of a block, without `\n` / `\r\n` terminators.
pub(super) struct Lines<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        let (line, rest) = memchr(b'\n', self.rest).map_or((self.rest, &[][..]), |i| {
            (&self.rest[..i], &self.rest[i + 1..])
        });
        self.rest = rest;
        Some(line.strip_suffix(b"\r").unwrap_or(line))
    }
}

pub(super) const fn lines(block: &[u8]) -> Lines<'_> {
    Lines { rest: block }
}

/// The record type of a line: its first tab-separated field (`#` for comments).
pub(super) fn record_type(line: &[u8]) -> &[u8] {
    if line.first() == Some(&b'#') {
        return b"#";
    }
    &line[..memchr(b'\t', line).unwrap_or(line.len())]
}

/// Logs each skipped record type once.
pub(super) fn note_skipped(seen: &mut Vec<Vec<u8>>, record_type: &[u8]) {
    if record_type != b"H" && !seen.iter().any(|t| t == record_type) {
        debug!("Skipping GFA records of type {:?}", lossy(record_type));
        seen.push(record_type.to_vec());
    }
}
