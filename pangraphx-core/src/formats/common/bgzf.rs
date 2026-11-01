//! Parallel BGZF decompression.
//!
//! BGZF (the SAM/BAM block gzip format, which libvgio writes by default) is a
//! series of gzip members of at most 64 KiB of input each. Every member
//! carries its compressed size in a `BC` extra field, so the stream splits
//! into blocks without inflating it, and blocks inflate independently.
//! [`BgzfReader`] reads a batch of blocks, inflates them in parallel into one
//! buffer in stream order, and serves reads from it.
//!
//! Block layout (little-endian):
//!
//! ```text
//! 1F 8B 08 04  MTIME(4) XFL OS  XLEN=6(2)  'B' 'C' SLEN=2(2) BSIZE(2)   18-byte header
//! CDATA (raw deflate, BSIZE − XLEN − 19 bytes)
//! CRC32(4) ISIZE(4)                                                      8-byte trailer
//! ```
//!
//! `BSIZE` is the whole block's size minus one. Like htslib, the reader
//! accepts only this header (`XLEN = 6`, the `BC` field alone).

use flate2::{Crc, Decompress, FlushDecompress, Status};
use rayon::prelude::*;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;

/// Size of a BGZF block header.
const HEADER_LEN: usize = 18;
/// Size of a block trailer: CRC32 and ISIZE.
const TRAILER_LEN: usize = 8;
/// Largest uncompressed size of a block.
const MAX_BLOCK_DATA: usize = 1 << 16;
/// Uncompressed bytes inflated per batch: over a hundred blocks, so every
/// thread gets several, while the buffers stay small next to a parsed graph.
pub const BATCH_LEN: usize = 8 << 20;

/// How a stream is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    /// gzip, but not BGZF: one stream that must be inflated serially.
    Gzip,
    Bgzf,
}

/// Whether `header` starts a BGZF block.
fn is_bgzf_header(header: &[u8]) -> bool {
    header.len() >= HEADER_LEN
        && header[..4] == [0x1F, 0x8B, 0x08, 0x04]
        && header[10..16] == [6, 0, b'B', b'C', 2, 0]
}

/// Detects the compression of the stream at the reader's position, which is
/// left unchanged.
///
/// # Errors
/// Returns an error if the reader fails to read or seek.
pub fn detect<R: Read + Seek>(reader: &mut R) -> io::Result<Compression> {
    let start = reader.stream_position()?;
    let mut header = Vec::with_capacity(HEADER_LEN);
    reader
        .by_ref()
        .take(HEADER_LEN as u64)
        .read_to_end(&mut header)?;
    reader.seek(SeekFrom::Start(start))?;
    Ok(if is_bgzf_header(&header) {
        Compression::Bgzf
    } else if header.starts_with(&[0x1F, 0x8B, 0x08]) {
        Compression::Gzip
    } else {
        Compression::None
    })
}

/// An error in the block at compressed byte `offset`.
fn invalid(offset: u64, message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("BGZF block at compressed byte {offset}: {message}"),
    )
}

/// One block of a batch.
struct Block {
    /// Its deflate data in [`BgzfReader::compressed`].
    data: Range<usize>,
    crc: u32,
    /// Uncompressed size.
    len: usize,
    /// Offset of the block in the compressed stream, for errors.
    offset: u64,
}

/// Reads a BGZF stream, inflating batches of blocks in parallel.
///
/// The buffers are reused across batches; see `vg::framing::read_window` for
/// why that matters on macOS.
pub struct BgzfReader<R> {
    inner: R,
    batch_len: usize,
    /// Deflate data of the current batch, back to back.
    compressed: Vec<u8>,
    blocks: Vec<Block>,
    /// The current batch, inflated.
    out: Vec<u8>,
    /// Bytes of `out` already read.
    pos: usize,
    /// Offset of the next block in the compressed stream.
    offset: u64,
    at_end: bool,
}

impl<R: Read> BgzfReader<R> {
    pub const fn new(inner: R) -> Self {
        Self::with_batch_len(inner, BATCH_LEN)
    }

    /// A reader inflating about `batch_len` bytes per batch.
    pub const fn with_batch_len(inner: R, batch_len: usize) -> Self {
        Self {
            inner,
            batch_len,
            compressed: Vec::new(),
            blocks: Vec::new(),
            out: Vec::new(),
            pos: 0,
            offset: 0,
            at_end: false,
        }
    }

    /// Reads the next block into `compressed`, or `None` at the end of the stream.
    fn read_block(&mut self) -> io::Result<Option<Block>> {
        let offset = self.offset;
        let mut header = [0u8; HEADER_LEN];
        let read = read_full(&mut self.inner, &mut header)?;
        if read == 0 {
            return Ok(None);
        }
        if read < HEADER_LEN {
            return Err(invalid(offset, "truncated header"));
        }
        if !is_bgzf_header(&header) {
            return Err(invalid(offset, "not a BGZF block header"));
        }
        let block_len = usize::from(u16::from_le_bytes([header[16], header[17]])) + 1;
        let Some(body_len) = block_len.checked_sub(HEADER_LEN + TRAILER_LEN) else {
            return Err(invalid(
                offset,
                &format!("block size {block_len} is too small"),
            ));
        };

        let start = self.compressed.len();
        let rest = body_len + TRAILER_LEN;
        let read = (&mut self.inner)
            .take(rest as u64)
            .read_to_end(&mut self.compressed)?;
        if read != rest {
            return Err(invalid(offset, "truncated block"));
        }
        let trailer = &self.compressed[start + body_len..];
        let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
        let len = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
        let len = usize::try_from(len).unwrap_or(usize::MAX);
        if len > MAX_BLOCK_DATA {
            return Err(invalid(
                offset,
                &format!("uncompressed size {len} exceeds 64 KiB"),
            ));
        }
        self.compressed.truncate(start + body_len);
        self.offset += block_len as u64;
        Ok(Some(Block {
            data: start..start + body_len,
            crc,
            len,
            offset,
        }))
    }

    /// Reads and inflates the next batch; `false` at the end of the stream.
    fn fill(&mut self) -> io::Result<bool> {
        self.compressed.clear();
        self.blocks.clear();
        let mut total = 0;
        while total < self.batch_len && !self.at_end {
            match self.read_block()? {
                Some(block) => {
                    total += block.len;
                    self.blocks.push(block);
                }
                None => self.at_end = true,
            }
        }
        if self.blocks.is_empty() {
            return Ok(false);
        }

        self.out.clear();
        self.out.resize(total, 0);
        let mut outputs = Vec::with_capacity(self.blocks.len());
        let mut rest = self.out.as_mut_slice();
        for block in &self.blocks {
            let (output, tail) = std::mem::take(&mut rest).split_at_mut(block.len);
            outputs.push(output);
            rest = tail;
        }
        let compressed = &self.compressed;
        let results: Vec<Result<(), String>> = self
            .blocks
            .par_iter()
            .zip(outputs)
            .map_init(
                || Decompress::new(false),
                |inflater, (block, output)| {
                    inflate(inflater, &compressed[block.data.clone()], output)?;
                    let mut crc = Crc::new();
                    crc.update(output);
                    if crc.sum() == block.crc {
                        Ok(())
                    } else {
                        Err("CRC32 mismatch".to_owned())
                    }
                },
            )
            .collect();
        // The first failing block in stream order, whatever the thread count.
        if let Some((block, Err(message))) = self
            .blocks
            .iter()
            .zip(results)
            .find(|(_, result)| result.is_err())
        {
            return Err(invalid(block.offset, &message));
        }
        self.pos = 0;
        Ok(true)
    }
}

/// Inflates the raw deflate stream `input`, which must fill `output` exactly.
fn inflate(inflater: &mut Decompress, input: &[u8], output: &mut [u8]) -> Result<(), String> {
    inflater.reset(false);
    let status = inflater
        .decompress(input, output, FlushDecompress::Finish)
        .map_err(|e| format!("invalid deflate data: {e}"))?;
    let consumed = inflater.total_in() == input.len() as u64;
    let produced = inflater.total_out() == output.len() as u64;
    if status == Status::StreamEnd && consumed && produced {
        Ok(())
    } else {
        Err("deflate data does not match the block's sizes".to_owned())
    }
}

/// Reads until `buf` is full or the stream ends; returns the bytes read.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

impl<R: Read> Read for BgzfReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // A batch can be empty (the end-of-file marker block holds no data).
        while self.pos == self.out.len() {
            if !self.fill()? {
                return Ok(0);
            }
        }
        let n = buf.len().min(self.out.len() - self.pos);
        buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Compresses `data` as BGZF blocks of at most `block_len` input bytes, with
/// the empty end-of-file block, as bgzip and htslib write it.
#[cfg(test)]
pub fn compress(data: &[u8], block_len: usize) -> Vec<u8> {
    use flate2::write::DeflateEncoder;
    use std::io::Write;

    let mut out = Vec::new();
    let mut write_block = |chunk: &[u8]| {
        let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(chunk).unwrap();
        let deflated = encoder.finish().unwrap();
        let block_size = u16::try_from(HEADER_LEN + deflated.len() + TRAILER_LEN - 1).unwrap();
        out.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x04, 0, 0, 0, 0, 0, 0xFF]);
        out.extend_from_slice(&[6, 0, b'B', b'C', 2, 0]);
        out.extend_from_slice(&block_size.to_le_bytes());
        out.extend_from_slice(&deflated);
        let mut crc = Crc::new();
        crc.update(chunk);
        out.extend_from_slice(&crc.sum().to_le_bytes());
        out.extend_from_slice(&u32::try_from(chunk.len()).unwrap().to_le_bytes());
    };
    for chunk in data.chunks(block_len) {
        write_block(chunk);
    }
    write_block(&[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from(i * 7 % 251).unwrap())
            .collect()
    }

    fn decompress(data: &[u8], batch_len: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        BgzfReader::with_batch_len(Cursor::new(data), batch_len).read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn test_round_trip() {
        for (len, block_len) in [(3000, 1), (300_000, 1000), (300_000, MAX_BLOCK_DATA)] {
            let data = sample(len);
            let compressed = compress(&data, block_len);
            // Batches of one block, a few blocks, and the whole stream.
            for batch_len in [1, 5000, usize::MAX] {
                let out = decompress(&compressed, batch_len).unwrap();
                assert!(out == data, "blocks of {block_len}, batches of {batch_len}");
            }
        }
        assert_eq!(decompress(&compress(&[], 10), 1).unwrap(), Vec::<u8>::new());
        assert_eq!(decompress(&[], 1).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn test_reads_of_one_byte() {
        let data = sample(5000);
        let mut reader = BgzfReader::with_batch_len(Cursor::new(compress(&data, 700)), 2000);
        let mut out = Vec::new();
        let mut byte = [0u8];
        while reader.read(&mut byte).unwrap() == 1 {
            out.push(byte[0]);
        }
        assert_eq!(out, data);
    }

    #[test]
    fn test_detect() {
        let bgzf = compress(b"data", 10);
        let mut cursor = Cursor::new([b"xx".as_slice(), &bgzf].concat());
        assert_eq!(detect(&mut cursor).unwrap(), Compression::None);
        cursor.set_position(2);
        assert_eq!(detect(&mut cursor).unwrap(), Compression::Bgzf);
        assert_eq!(cursor.position(), 2);

        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gzip, b"data").unwrap();
        let gzip = gzip.finish().unwrap();
        assert_eq!(detect(&mut Cursor::new(gzip)).unwrap(), Compression::Gzip);
        assert_eq!(
            detect(&mut Cursor::new(&bgzf[..3])).unwrap(),
            Compression::Gzip
        );
        assert_eq!(detect(&mut Cursor::new(b"")).unwrap(), Compression::None);
    }

    fn error(data: &[u8]) -> String {
        decompress(data, 1).unwrap_err().to_string()
    }

    #[test]
    fn test_corrupt_streams_are_rejected() {
        let data = sample(3000);
        let compressed = compress(&data, 1000);
        let second = usize::from(u16::from_le_bytes([compressed[16], compressed[17]])) + 1;

        assert!(error(&compressed[..compressed.len() - 3]).contains("truncated block"));
        assert!(error(&compressed[..second + 5]).contains("truncated header"));

        let mut flipped = compressed.clone();
        flipped[second + 20] ^= 0xFF;
        let message = error(&flipped);
        assert!(
            message.contains(&format!("compressed byte {second}")),
            "{message}"
        );

        let mut bad_crc = compressed.clone();
        bad_crc[second - TRAILER_LEN] ^= 1;
        assert!(error(&bad_crc).contains("CRC32 mismatch"));

        let mut bad_len = compressed.clone();
        bad_len[second - 4] ^= 1; // ISIZE of the first block
        assert!(error(&bad_len).contains("does not match"));

        let mut plain_gzip = compressed;
        plain_gzip[second + 3] = 0; // FLG: no extra field
        assert!(error(&plain_gzip).contains("not a BGZF block header"));
    }
}
