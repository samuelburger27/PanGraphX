//! libvgio framing: groups of varint-length-prefixed messages, the first of
//! which may be a type tag.

use crate::error::{PanGraphXError, PanResult};
use log::debug;
use prost::Message;
use std::io::{Read, Write};
use std::ops::Range;

#[cfg(test)]
use super::reader::VG_READ_WINDOW;
#[cfg(test)]
use crate::proto_gen::vg as vg_proto;

/// Reads a varint64 from a byte stream. Returns `None` at EOF.
pub(super) fn read_varint<R: Read>(reader: &mut R) -> PanResult<Option<u64>> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    let mut buf = [0u8; 1];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                if shift == 0 {
                    return Ok(None); // clean EOF
                }
                return Err(PanGraphXError::Parse(
                    "Unexpected EOF while reading varint".to_string(),
                ));
            }
            Ok(_) => {
                let byte = buf[0];
                value |= u64::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    return Ok(Some(value));
                }
                shift += 7;
                if shift >= 64 {
                    return Err(PanGraphXError::Parse("Varint too long".to_string()));
                }
            }
            Err(e) => return Err(PanGraphXError::Io(e)),
        }
    }
}

/// Writes a varint64 to a byte stream.
pub(super) fn write_varint(writer: &mut dyn Write, mut value: u64) -> PanResult<()> {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        writer.write_all(&[byte])?;
        if value == 0 {
            break;
        }
    }
    Ok(())
}

/// Largest message length accepted when reading: protobuf messages are limited
/// to 2 GB, so a larger length means a corrupt stream, not a huge allocation.
pub(super) const VG_MAX_MESSAGE_LEN: u64 = i32::MAX as u64;

/// Raw `Graph` messages, stored back to back.
#[derive(Default)]
pub(super) struct RawWindow {
    pub(super) bytes: Vec<u8>,
    pub(super) messages: Vec<Range<usize>>,
}

impl RawWindow {
    /// The bytes of the message at `range`, one of `self.messages`.
    pub(super) fn message(&self, range: &Range<usize>) -> &[u8] {
        &self.bytes[range.clone()]
    }
}

/// Splits a VG-framed protobuf stream into its `Graph` messages.
///
/// The VG framing format consists of groups. Each group starts with a varint
/// count of messages, each message is length-prefixed with a varint. The first
/// message in a group may be a type tag string (e.g. `"VG"`), which is skipped.
pub(super) struct VgFrames<'r, R> {
    reader: &'r mut R,
    /// Messages left in the current group.
    remaining: u64,
    /// Index of the next message within its group.
    index: u64,
}

impl<'r, R: Read> VgFrames<'r, R> {
    const fn new(reader: &'r mut R) -> Self {
        Self {
            reader,
            remaining: 0,
            index: 0,
        }
    }

    /// Replaces the contents of `window` with the next messages, until they
    /// hold at least `budget` bytes or the stream ends; an empty window means
    /// the stream has ended. Windows are reused: allocating a fresh buffer per
    /// window left the freed ones resident in the macOS allocator and doubled
    /// the peak memory of a chr22 load.
    fn read_window(&mut self, budget: usize, window: &mut RawWindow) -> PanResult<()> {
        window.bytes.clear();
        window.messages.clear();
        while window.bytes.len() < budget {
            if self.remaining == 0 {
                let Some(count) = read_varint(self.reader)? else {
                    break;
                };
                self.remaining = count;
                self.index = 0;
                continue;
            }
            self.remaining -= 1;
            let index = self.index;
            self.index += 1;

            let msg_len = read_varint(self.reader)?.ok_or_else(|| {
                PanGraphXError::Parse("Unexpected EOF reading message length".to_string())
            })?;
            if msg_len > VG_MAX_MESSAGE_LEN {
                return Err(PanGraphXError::Parse(format!(
                    "VG message length {msg_len} exceeds the protobuf limit of {VG_MAX_MESSAGE_LEN} bytes"
                )));
            }
            // Reading through `take` fills the buffer without zeroing it first,
            // and grows it only as bytes arrive, so a corrupt length cannot
            // allocate more than the stream holds.
            let start = window.bytes.len();
            let read = (&mut *self.reader)
                .take(msg_len)
                .read_to_end(&mut window.bytes)?;
            if u64::try_from(read).ok() != Some(msg_len) {
                return Err(PanGraphXError::Parse(format!(
                    "Unexpected EOF in a VG message: expected {msg_len} bytes, found {read}"
                )));
            }

            // Type tags are short ASCII strings; Graph messages are protobuf-encoded.
            if index == 0 {
                let (is_type_tag, tag_str) = detect_type_tag(&window.bytes[start..]);
                if is_type_tag {
                    debug!("VG stream type tag: {tag_str:?}");
                    window.bytes.truncate(start);
                    continue;
                }
            }
            window.messages.push(start..window.bytes.len());
        }
        Ok(())
    }
}

/// Reads a VG-framed stream in windows of about `budget` bytes and runs
/// `process` on each window while the next one is read; `consume` receives
/// the results in stream order.
///
/// `in_place_scope` keeps the reader on this thread, so it need not be `Send`.
pub(super) fn for_each_window<R: Read, T: Default + Send>(
    reader: &mut R,
    budget: usize,
    process: impl Fn(&RawWindow) -> PanResult<T> + Sync,
    mut consume: impl FnMut(T),
) -> PanResult<()> {
    let mut frames = VgFrames::new(reader);
    let mut current = RawWindow::default();
    let mut next = RawWindow::default();
    frames.read_window(budget, &mut current)?;
    while !current.messages.is_empty() {
        let mut processed = Ok(T::default());
        let read = rayon::in_place_scope(|scope| {
            scope.spawn(|_| processed = process(&current));
            frames.read_window(budget, &mut next)
        });
        consume(processed?);
        read?;
        std::mem::swap(&mut current, &mut next);
    }
    Ok(())
}

/// Reads all `Graph` messages from a VG-framed protobuf stream.
#[cfg(test)]
pub(super) fn read_vg_graphs<R: Read>(reader: &mut R) -> PanResult<Vec<vg_proto::Graph>> {
    let mut graphs = Vec::new();
    for_each_window(
        reader,
        VG_READ_WINDOW,
        |window| {
            window
                .messages
                .iter()
                .map(|range| decode_graph(window.message(range)))
                .collect::<PanResult<Vec<_>>>()
        },
        |decoded| graphs.extend(decoded),
    )?;
    Ok(graphs)
}

/// Decodes one `Graph` message as `M` (the full `vg_proto::Graph` or the
/// reader's `vg_read::Graph`).
pub(super) fn decode_graph<M: Message + Default>(message: &[u8]) -> PanResult<M> {
    M::decode(message)
        .map_err(|e| PanGraphXError::Parse(format!("Failed to decode VG Graph message: {e}")))
}

/// Heuristic to determine if a message buffer is a type tag string.
/// Type tags are short (typically <= 10 bytes), all printable ASCII.
pub(super) fn detect_type_tag(buf: &[u8]) -> (bool, String) {
    if buf.len() <= 16 && !buf.is_empty() && buf.iter().all(|&b| b.is_ascii_graphic()) {
        let s = String::from_utf8_lossy(buf).to_string();
        return (true, s);
    }
    (false, String::new())
}

/// Writes one framing group: the `"VG"` type tag followed by already-encoded `Graph` messages.
pub(super) fn write_vg_group(writer: &mut dyn Write, encoded_graphs: &[Vec<u8>]) -> PanResult<()> {
    let count = encoded_graphs.len() + 1; // +1 for type tag
    write_varint(writer, count as u64)?;

    let tag = b"VG";
    write_varint(writer, tag.len() as u64)?;
    writer.write_all(tag)?;

    for encoded in encoded_graphs {
        write_varint(writer, encoded.len() as u64)?;
        writer.write_all(encoded)?;
    }

    Ok(())
}

/// Writes `Graph` messages to a VG-framed protobuf stream as a single group.
#[cfg(test)]
pub(super) fn write_vg_graphs(writer: &mut dyn Write, graphs: &[vg_proto::Graph]) -> PanResult<()> {
    let encoded: Vec<Vec<u8>> = graphs.iter().map(Message::encode_to_vec).collect();
    write_vg_group(writer, &encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ---- Varint tests ----

    #[test]
    fn test_varint_roundtrip() {
        let values: Vec<u64> = vec![
            0,
            1,
            127,
            128,
            255,
            300,
            16384,
            u64::from(u32::MAX),
            u64::MAX,
        ];
        for &val in &values {
            let mut buf = Vec::new();
            write_varint(&mut buf, val).unwrap();
            let mut cursor = Cursor::new(&buf);
            let decoded = read_varint(&mut cursor).unwrap().unwrap();
            assert_eq!(val, decoded, "Varint round-trip failed for {val}");
        }
    }

    #[test]
    fn test_varint_eof_returns_none() {
        let buf: Vec<u8> = vec![];
        let mut cursor = Cursor::new(&buf);
        assert!(read_varint(&mut cursor).unwrap().is_none());
    }

    // ---- Framing tests ----

    #[test]
    fn test_framing_roundtrip() {
        let graph = vg_proto::Graph {
            node: vec![
                vg_proto::Node {
                    id: 1,
                    sequence: "ACGT".to_string(),
                    name: String::new(),
                },
                vg_proto::Node {
                    id: 2,
                    sequence: "TTGG".to_string(),
                    name: String::new(),
                },
            ],
            edge: vec![vg_proto::Edge {
                from: 1,
                to: 2,
                from_start: false,
                to_end: false,
                overlap: 0,
            }],
            path: vec![],
        };

        let mut buf = Vec::new();
        write_vg_graphs(&mut buf, std::slice::from_ref(&graph)).unwrap();

        let mut cursor = Cursor::new(&buf);
        let decoded = read_vg_graphs(&mut cursor).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].node.len(), 2);
        assert_eq!(decoded[0].edge.len(), 1);
        assert_eq!(decoded[0].node[0].id, 1);
        assert_eq!(decoded[0].node[0].sequence, "ACGT");
    }

    #[test]
    fn test_framing_multiple_groups() {
        let g1 = vg_proto::Graph {
            node: vec![vg_proto::Node {
                id: 1,
                sequence: "AA".to_string(),
                name: String::new(),
            }],
            edge: vec![],
            path: vec![],
        };
        let g2 = vg_proto::Graph {
            node: vec![vg_proto::Node {
                id: 2,
                sequence: "CC".to_string(),
                name: String::new(),
            }],
            edge: vec![],
            path: vec![],
        };

        let mut buf = Vec::new();
        write_vg_graphs(&mut buf, &[g1]).unwrap();
        write_vg_graphs(&mut buf, &[g2]).unwrap();

        let mut cursor = Cursor::new(&buf);
        let decoded = read_vg_graphs(&mut cursor).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].node[0].sequence, "AA");
        assert_eq!(decoded[1].node[0].sequence, "CC");
    }

    #[test]
    fn test_oversized_message_length_is_rejected() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 1).unwrap();
        write_varint(&mut buf, u64::MAX >> 1).unwrap();
        let err = read_vg_graphs(&mut Cursor::new(buf)).unwrap_err();
        assert!(
            err.to_string().contains("exceeds the protobuf limit"),
            "{err}"
        );
    }
}
