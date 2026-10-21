//! Native reader and writer for vg's `PackedGraph` format (`.pg`), which current
//! vg also writes by default into `.vg` files (`vg convert`, `vg construct`).
//!
//! The layout mirrors libbdsg's `BasePackedGraph::serialize_members`. All
//! integers are little-endian except the leading magic number, and every
//! vector is built on an sdsl `int_vector<>`:
//!
//! ```text
//! u32 BE   magic (3080648541)
//! i64 ×2   max_id, min_id
//! paged    graph            per node record: start-side edge list, end-side edge list
//! paged    seq_start        per record: offset of its sequence in seq
//! packed   seq_length       per record: sequence length
//! paged*   edge_lists       per edge record: target handle, next edge record
//! deque    id_to_record     id - min_id → record (0: no node)
//! packed   seq              bases, ACGTN as 0..=4
//! paged    membership_node  per record: first membership of the node
//! paged*   membership_path  per membership: path index
//! paged    membership_step  per membership: step of the node on that path
//! paged    membership_next  per membership: next membership of the same node
//! string   alphabet         u64 length, the distinct path name bytes
//! packed   path_names       all path names, as indices into the alphabet
//! paged    path_name_start, packed path_name_length
//! packed   path_is_deleted, path_is_circular
//! paged*   path_head, paged path_tail      first and last step of each path
//! packed   path_deleted_steps
//! per path: robust links (previous step, next step), robust steps (handle)
//! u64 ×6   deleted node, edge and membership records, deleted bases,
//!          reversing self-edges, deleted reversing self-edges
//!
//! int_vector  u64 bit length, u8 width, ⌈bits / 64⌉ words; values span words
//! packed      u64 length, int_vector (may hold more values than the length)
//! paged       u64 length, u64 page size (256, 1024 for paged*), packed
//!             anchors, one packed page per anchor; a stored v ≠ 0 decodes to
//!             anchor − v/5 if 5 divides v, else anchor + v − v/5 − 1
//! deque       u64 begin, u64 length, packed ring buffer
//! robust      packed first page (256 values), paged rest
//! ```
//!
//! Records, edge records, memberships and steps are numbered from 1, so that
//! 0 ends a linked list. A handle is `id << 1 | is_reverse`. The edge `a → b`
//! is stored as `b` in the list of the side of `a` it leaves (the end side
//! when `a` is forward) and as `flip(a)` in the list of the side of `b` it
//! enters; a reversing self-edge (`a+ → a-`) has a single record. Graphs that
//! vg edited may keep unlinked (deleted) records, so the reader only follows
//! the lists.
//!
//! Sequences are restricted to ACGTN: other bytes are written as `N` and
//! lowercase is read back as uppercase. Overlaps are not part of the format.

use crate::core::core_types::{Edge, NodeId, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::binio::{self, MAX_PREALLOC};
use crate::formats::id_map::Numbering;
use crate::formats::vg_format::parse_vg_family;
use crate::traits::{GraphParser, GraphSerializer};
use log::warn;
use rayon::prelude::*;
use rustc_hash::FxHashSet;
use std::collections::HashMap;
use std::io::{BufWriter, Read, Seek, Write};

/// Codec for vg's `PackedGraph` format.
///
/// Parsing accepts everything a `.vg` file may hold (`PackedGraph`, `HashGraph`,
/// Protobuf). Node ids are written as in ODGI: numeric node names are kept
/// when they are distinct positive integers that fit a dense id range,
/// otherwise nodes are numbered `1..=n`. Parsed nodes are named by their id.
pub struct PackedGraphCodec;

/// libhandlegraph magic number of `bdsg::PackedGraph`.
pub const PACKED_GRAPH_MAGIC: u32 = 3_080_648_541;

/// Magic number of `bdsg::MappedPackedGraph`, which this crate does not read.
pub const MAPPED_PACKED_GRAPH_MAGIC: u32 = 672_226_447;

/// Format name used in error messages.
const FORMAT: &str = "PackedGraph";

/// Page sizes of libbdsg's paged vectors.
const NARROW_PAGE: usize = 256;
const WIDE_PAGE: usize = 1024;

/// Decoding of the 3-bit bases.
const ALPHABET: &[u8; 5] = b"ACGTN";

/// Marks a record without a node.
const VACANT: NodeId = NodeId::MAX;

/// Whether `from → to` (by handle) is the copy of an edge to keep when reading.
///
/// Both sides of an edge list it, once as the edge and once as its reverse
/// complement; the smaller of the two is kept. A reversing self-edge is its
/// own reverse complement and listed once.
#[must_use]
pub const fn is_canonical_copy(from: u64, to: u64) -> bool {
    from < (to ^ 1) || (from == (to ^ 1) && to <= (from ^ 1))
}

fn read_u64<R: Read>(reader: &mut R) -> PanResult<u64> {
    Ok(u64::from_le_bytes(binio::read_array(reader, FORMAT)?))
}

fn to_usize(value: u64, what: &str) -> PanResult<usize> {
    binio::to_usize(value, FORMAT, what)
}

fn out_of_range(what: &str) -> PanGraphXError {
    PanGraphXError::Parse(format!("{FORMAT} {what} is out of range"))
}

/// Decodes a paged-vector entry stored relative to its page anchor.
const fn from_diff(diff: u64, anchor: u64) -> u64 {
    if diff == 0 {
        0
    } else if diff.is_multiple_of(5) {
        anchor.wrapping_sub(diff / 5)
    } else {
        anchor.wrapping_add(diff - diff / 5 - 1)
    }
}

/// Inverse of [`from_diff`]: small for values slightly above the anchor.
const fn to_diff(value: u64, anchor: u64) -> u64 {
    if value == 0 {
        0
    } else if value >= anchor {
        let raw = value - anchor;
        raw + raw / 4 + 1
    } else {
        5 * (anchor - value)
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// An sdsl `int_vector<>`: `len` values of `width` bits, packed back to back.
struct IntVector {
    words: Vec<u64>,
    len: usize,
    width: usize,
    mask: u64,
}

impl IntVector {
    fn read<R: Read>(reader: &mut R) -> PanResult<Self> {
        let bits = read_u64(reader)?;
        let width = binio::read_array::<1, _>(reader, FORMAT)?[0];
        if width == 0 || width > 64 {
            return Err(PanGraphXError::Parse(format!(
                "Invalid {FORMAT} integer width {width}"
            )));
        }
        let words = binio::read_words_le(reader, bits.div_ceil(64), FORMAT)?;
        Ok(Self {
            words,
            len: to_usize(bits / u64::from(width), "vector length")?,
            width: usize::from(width),
            mask: u64::MAX >> (64 - width),
        })
    }

    /// Value at `index`; `index` must be below `len`, which keeps every word
    /// accessed within the `⌈len · width / 64⌉` words read.
    fn get(&self, index: usize) -> u64 {
        let bit = index * self.width;
        let (word, offset) = (bit / 64, bit % 64);
        let mut value = self.words[word] >> offset;
        if offset + self.width > 64 {
            value |= self.words[word + 1] << (64 - offset);
        }
        value & self.mask
    }
}

/// libbdsg `PackedVector`: the first `len` values of an int vector.
struct PackedVector {
    len: usize,
    values: IntVector,
}

impl PackedVector {
    fn read<R: Read>(reader: &mut R) -> PanResult<Self> {
        let len = to_usize(read_u64(reader)?, "vector length")?;
        let values = IntVector::read(reader)?;
        if len > values.len {
            return Err(PanGraphXError::Parse(format!(
                "{FORMAT} vector is shorter than its length"
            )));
        }
        Ok(Self { len, values })
    }

    fn get(&self, index: usize) -> PanResult<u64> {
        if index < self.len {
            Ok(self.values.get(index))
        } else {
            Err(out_of_range("vector index"))
        }
    }
}

/// libbdsg `PagedVector`: fixed-size pages stored relative to an anchor each.
struct PagedVector {
    len: usize,
    page: usize,
    anchors: PackedVector,
    pages: Vec<PackedVector>,
}

impl PagedVector {
    fn read<R: Read>(reader: &mut R, page: usize) -> PanResult<Self> {
        let len = to_usize(read_u64(reader)?, "vector length")?;
        let stored_page = read_u64(reader)?;
        if stored_page != page as u64 {
            return Err(PanGraphXError::Parse(format!(
                "{FORMAT} paged vector has page size {stored_page}, expected {page}"
            )));
        }
        let anchors = PackedVector::read(reader)?;
        let mut pages = Vec::with_capacity(anchors.len.min(MAX_PREALLOC));
        for _ in 0..anchors.len {
            pages.push(PackedVector::read(reader)?);
        }
        if len.div_ceil(page) > pages.len() {
            return Err(PanGraphXError::Parse(format!(
                "{FORMAT} paged vector has fewer pages than its length needs"
            )));
        }
        Ok(Self {
            len,
            page,
            anchors,
            pages,
        })
    }

    fn get(&self, index: usize) -> PanResult<u64> {
        if index >= self.len {
            return Err(out_of_range("vector index"));
        }
        let page = index / self.page;
        let diff = self.pages[page].get(index % self.page)?;
        Ok(from_diff(diff, self.anchors.get(page)?))
    }
}

/// libbdsg `PackedDeque`: a ring buffer starting at `begin`.
struct PackedDeque {
    begin: usize,
    len: usize,
    ring: PackedVector,
}

impl PackedDeque {
    fn read<R: Read>(reader: &mut R) -> PanResult<Self> {
        let begin = to_usize(read_u64(reader)?, "deque start")?;
        let len = to_usize(read_u64(reader)?, "deque length")?;
        let ring = PackedVector::read(reader)?;
        if len > ring.len || (len > 0 && begin >= ring.len) {
            return Err(PanGraphXError::Parse(format!(
                "{FORMAT} deque does not fit its buffer"
            )));
        }
        Ok(Self { begin, len, ring })
    }

    fn get(&self, index: usize) -> PanResult<u64> {
        if index >= self.len {
            return Err(out_of_range("deque index"));
        }
        let to_end = self.ring.len - self.begin;
        self.ring.get(if index < to_end {
            self.begin + index
        } else {
            index - to_end
        })
    }
}

/// libbdsg `RobustPagedVector`: the first page unpaged, then a paged vector.
struct RobustPagedVector {
    first: PackedVector,
    rest: PagedVector,
}

impl RobustPagedVector {
    fn read<R: Read>(reader: &mut R) -> PanResult<Self> {
        Ok(Self {
            first: PackedVector::read(reader)?,
            rest: PagedVector::read(reader, NARROW_PAGE)?,
        })
    }

    const fn len(&self) -> usize {
        self.first.len + self.rest.len
    }

    fn get(&self, index: usize) -> PanResult<u64> {
        if index < NARROW_PAGE {
            self.first.get(index)
        } else {
            self.rest.get(index - NARROW_PAGE)
        }
    }
}

/// The parts of a `PackedGraph` file needed to rebuild the graph.
struct PackedGraphFile {
    min_id: i64,
    graph: PagedVector,
    seq_start: PagedVector,
    seq_length: PackedVector,
    edge_lists: PagedVector,
    id_to_record: PackedDeque,
    seq: PackedVector,
    alphabet: Vec<u8>,
    path_names: PackedVector,
    path_name_start: PagedVector,
    path_name_length: PackedVector,
    path_is_deleted: PackedVector,
    path_is_circular: PackedVector,
    path_head: PagedVector,
    /// Links and steps of every path.
    paths: Vec<(RobustPagedVector, RobustPagedVector)>,
}

impl PackedGraphFile {
    /// Reads everything after the magic number.
    fn read<R: Read>(reader: &mut R) -> PanResult<Self> {
        let _max_id = read_u64(reader)?;
        let min_id = read_u64(reader)?.cast_signed();
        let graph = PagedVector::read(reader, NARROW_PAGE)?;
        let seq_start = PagedVector::read(reader, NARROW_PAGE)?;
        let seq_length = PackedVector::read(reader)?;
        let edge_lists = PagedVector::read(reader, WIDE_PAGE)?;
        let id_to_record = PackedDeque::read(reader)?;
        let seq = PackedVector::read(reader)?;
        // Path memberships only index the paths, which are walked directly.
        let _membership_node = PagedVector::read(reader, NARROW_PAGE)?;
        let _membership_path = PagedVector::read(reader, WIDE_PAGE)?;
        let _membership_step = PagedVector::read(reader, NARROW_PAGE)?;
        let _membership_next = PagedVector::read(reader, NARROW_PAGE)?;
        let alphabet_len = read_u64(reader)?;
        let alphabet = binio::read_bytes(reader, alphabet_len, FORMAT)?;
        let path_names = PackedVector::read(reader)?;
        let path_name_start = PagedVector::read(reader, NARROW_PAGE)?;
        let path_name_length = PackedVector::read(reader)?;
        let path_is_deleted = PackedVector::read(reader)?;
        let path_is_circular = PackedVector::read(reader)?;
        let path_head = PagedVector::read(reader, WIDE_PAGE)?;
        let _path_tail = PagedVector::read(reader, NARROW_PAGE)?;
        let _path_deleted_steps = PackedVector::read(reader)?;
        let path_count = path_name_start.len;
        let mut paths = Vec::with_capacity(path_count.min(MAX_PREALLOC));
        for _ in 0..path_count {
            let links = RobustPagedVector::read(reader)?;
            let steps = RobustPagedVector::read(reader)?;
            paths.push((links, steps));
        }
        // Deletion and reversing self-edge counters.
        for _ in 0..6 {
            read_u64(reader)?;
        }
        Ok(Self {
            min_id,
            graph,
            seq_start,
            seq_length,
            edge_lists,
            id_to_record,
            seq,
            alphabet,
            path_names,
            path_name_start,
            path_name_length,
            path_is_deleted,
            path_is_circular,
            path_head,
            paths,
        })
    }

    /// The 0-based record of node `id`, if the graph has that node.
    fn record_of(&self, id: u64) -> PanResult<Option<usize>> {
        let Some(index) = id
            .checked_sub(self.min_id.cast_unsigned())
            .filter(|_| id.cast_signed() >= self.min_id)
        else {
            return Ok(None);
        };
        let index = to_usize(index, "node id")?;
        if index >= self.id_to_record.len {
            return Ok(None);
        }
        match self.id_to_record.get(index)? {
            0 => Ok(None),
            record => Ok(Some(to_usize(record - 1, "node record")?)),
        }
    }

    fn sequence(&self, record: usize) -> PanResult<Vec<u8>> {
        let start = to_usize(self.seq_start.get(record)?, "sequence start")?;
        let len = to_usize(self.seq_length.get(record)?, "sequence length")?;
        let end = start
            .checked_add(len)
            .filter(|&end| end <= self.seq.len)
            .ok_or_else(|| out_of_range("node sequence"))?;
        (start..end)
            .map(|i| {
                let code = self.seq.get(i)?;
                ALPHABET
                    .get(to_usize(code, "base")?)
                    .copied()
                    .ok_or_else(|| PanGraphXError::Parse(format!("Invalid {FORMAT} base {code}")))
            })
            .collect()
    }

    fn path_name(&self, path: usize) -> PanResult<Vec<u8>> {
        let start = to_usize(self.path_name_start.get(path)?, "path name start")?;
        let len = to_usize(self.path_name_length.get(path)?, "path name length")?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| out_of_range("path name"))?;
        (start..end)
            .map(|i| {
                let code = to_usize(self.path_names.get(i)?, "path name character")?;
                self.alphabet
                    .get(code)
                    .copied()
                    .ok_or_else(|| out_of_range("path name character"))
            })
            .collect()
    }

    fn into_dto(self) -> PanResult<CoreGraphDTO> {
        let record_count = self.graph.len / 2;

        // Nodes in id order: the id-to-record table is indexed by id.
        let mut record_to_node = vec![VACANT; record_count];
        let mut records = Vec::new();
        let mut ids = Vec::new();
        for index in 0..self.id_to_record.len {
            let record = self.id_to_record.get(index)?;
            if record == 0 {
                continue;
            }
            let record = to_usize(record - 1, "node record")?;
            let slot = record_to_node
                .get_mut(record)
                .ok_or_else(|| out_of_range("node record"))?;
            if *slot != VACANT {
                return Err(PanGraphXError::Parse(format!(
                    "{FORMAT} node record {record} belongs to two ids"
                )));
            }
            *slot = records.len();
            let id = i64::try_from(index)
                .ok()
                .and_then(|index| self.min_id.checked_add(index))
                .filter(|&id| id > 0)
                .ok_or_else(|| {
                    PanGraphXError::Parse(format!("{FORMAT} node ids must be positive"))
                })?;
            records.push(record);
            ids.push(id.cast_unsigned());
        }

        let sequences = records
            .par_iter()
            .map(|&record| self.sequence(record))
            .collect::<PanResult<Vec<_>>>()?;

        let node_of = |handle: u64| -> PanResult<(NodeId, Orientation)> {
            let node = self
                .record_of(handle >> 1)?
                .and_then(|record| record_to_node.get(record).copied())
                .filter(|&node| node != VACANT)
                .ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "{FORMAT} references missing node id {}",
                        handle >> 1
                    ))
                })?;
            Ok((node, (handle & 1 == 1).into()))
        };

        let edges = records
            .par_iter()
            .enumerate()
            .map(|(node, &record)| self.node_edges(node, ids[node], record, &node_of))
            .collect::<PanResult<Vec<Vec<Edge>>>>()?
            .into_iter()
            .flatten()
            .collect();

        let paths = (0..self.paths.len())
            .into_par_iter()
            .map(|path| self.path(path, &node_of))
            .collect::<PanResult<Vec<Option<Path>>>>()?
            .into_iter()
            .flatten()
            .collect();

        let node_name_map = ids
            .iter()
            .enumerate()
            .map(|(node, &id)| (node, itoa::Buffer::new().format(id).as_bytes().to_vec()))
            .collect::<HashMap<_, _>>();
        Ok(CoreGraphDTO {
            nodes: Nodes::from_seq(sequences),
            edges,
            paths,
            node_name_map: Some(node_name_map),
        })
    }

    /// The edges of `node` (id `id`, record `record`) that are kept when
    /// reading, see [`is_canonical_copy`].
    fn node_edges(
        &self,
        node: NodeId,
        id: u64,
        record: usize,
        node_of: &(impl Fn(u64) -> PanResult<(NodeId, Orientation)> + Sync),
    ) -> PanResult<Vec<Edge>> {
        let edge_records = self.edge_lists.len / 2;
        let mut edges = Vec::new();
        // The end side lists edges leaving the node forward, the start side
        // edges leaving it in reverse.
        for (side, reverse) in [(1, false), (0, true)] {
            let from = id << 1 | u64::from(reverse);
            let mut next = self.graph.get(2 * record + side)?;
            let mut visited = 0;
            while next != 0 {
                visited += 1;
                if visited > edge_records {
                    return Err(PanGraphXError::Parse(format!(
                        "{FORMAT} edge list of node {id} does not end"
                    )));
                }
                let at = to_usize(next - 1, "edge record")?
                    .checked_mul(2)
                    .ok_or_else(|| out_of_range("edge record"))?;
                let to = self.edge_lists.get(at)?;
                next = self.edge_lists.get(at + 1)?;
                if is_canonical_copy(from, to) {
                    let (to_node, to_orient) = node_of(to)?;
                    edges.push(Edge {
                        from_node: node,
                        from_orient: reverse.into(),
                        to_node,
                        to_orient,
                        overlap: 0,
                    });
                }
            }
        }
        Ok(edges)
    }

    /// Walks path `path` from its head; `None` for a deleted path.
    fn path(
        &self,
        path: usize,
        node_of: &(impl Fn(u64) -> PanResult<(NodeId, Orientation)> + Sync),
    ) -> PanResult<Option<Path>> {
        if self.path_is_deleted.get(path)? != 0 {
            return Ok(None);
        }
        let name = self.path_name(path)?;
        let circular = self.path_is_circular.get(path)? != 0;
        if circular {
            warn!(
                "{FORMAT} path {} is circular; reading it as linear",
                String::from_utf8_lossy(&name)
            );
        }
        let (links, steps) = &self.paths[path];
        let head = self.path_head.get(path)?;
        let mut walked = Vec::new();
        let mut next = head;
        while next != 0 {
            if walked.len() >= steps.len() {
                return Err(PanGraphXError::Parse(format!(
                    "{FORMAT} path {} does not end",
                    String::from_utf8_lossy(&name)
                )));
            }
            let at = to_usize(next - 1, "path step")?;
            let (node, orientation) = node_of(steps.get(at)?)?;
            walked.push(Step::new(node, orientation)?);
            next = links.get(
                at.checked_mul(2)
                    .and_then(|i| i.checked_add(1))
                    .ok_or_else(|| out_of_range("path step"))?,
            )?;
            if circular && next == head {
                break;
            }
        }
        Ok(Some(Path {
            name,
            steps: walked,
            overlaps: Vec::new(),
        }))
    }
}

/// Reads a bare `PackedGraph` file, starting at its magic number.
///
/// # Errors
/// Returns a parse error for a truncated or malformed file, or an I/O error.
pub fn read_packed_graph<R: Read>(reader: &mut R) -> PanResult<CoreGraphDTO> {
    let magic = u32::from_be_bytes(binio::read_array(reader, FORMAT)?);
    if magic != PACKED_GRAPH_MAGIC {
        return Err(PanGraphXError::Parse(format!(
            "Not a {FORMAT} file (magic number {magic})"
        )));
    }
    PackedGraphFile::read(reader)?.into_dto()
}

impl<R: Read + Seek> GraphParser<R> for PackedGraphCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        parse_vg_family(reader)
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Smallest int vector width that holds `max` (sdsl widths start at 1).
const fn width_for(max: u64) -> u32 {
    let bits = 64 - max.leading_zeros();
    if bits == 0 { 1 } else { bits }
}

/// Writes an sdsl `int_vector<>` of `len` values of `width` bits.
fn write_int_vector<W: Write>(
    out: &mut W,
    len: usize,
    width: u32,
    values: impl Iterator<Item = u64>,
) -> PanResult<()> {
    out.write_all(&(len as u64 * u64::from(width)).to_le_bytes())?;
    #[allow(clippy::cast_possible_truncation)] // width is at most 64
    out.write_all(&[width as u8])?;
    let mut buf = Vec::with_capacity(8 * 1024);
    let (mut word, mut used) = (0_u64, 0_u32);
    for value in values {
        word |= value << used;
        let free = 64 - used;
        if width >= free {
            buf.extend_from_slice(&word.to_le_bytes());
            word = if free == 64 { 0 } else { value >> free };
            used = width - free;
            if buf.len() >= 8 * 1024 {
                out.write_all(&buf)?;
                buf.clear();
            }
        } else {
            used += width;
        }
    }
    if used > 0 {
        buf.extend_from_slice(&word.to_le_bytes());
    }
    out.write_all(&buf)?;
    Ok(())
}

/// Writes a `PackedVector` of the smallest width that holds `values`.
fn write_packed<W: Write>(out: &mut W, values: impl Iterator<Item = u64> + Clone) -> PanResult<()> {
    let (len, max) = values
        .clone()
        .fold((0_usize, 0_u64), |(len, max), v| (len + 1, max.max(v)));
    out.write_all(&(len as u64).to_le_bytes())?;
    write_int_vector(out, len, width_for(max), values)
}

/// Writes a `PagedVector` with pages of `page` values. The anchor of a page
/// is its first non-zero value; libbdsg keeps the first value ever written to
/// the page, but any anchor decodes the same.
fn write_paged<W: Write>(
    out: &mut W,
    page: usize,
    values: impl Iterator<Item = u64> + Clone,
) -> PanResult<()> {
    let mut len = 0_usize;
    let mut anchors = Vec::new();
    for value in values.clone() {
        if len.is_multiple_of(page) {
            anchors.push(0);
        }
        if let Some(anchor @ 0) = anchors.last_mut() {
            *anchor = value;
        }
        len += 1;
    }
    out.write_all(&(len as u64).to_le_bytes())?;
    out.write_all(&(page as u64).to_le_bytes())?;
    write_packed(out, anchors.iter().copied())?;
    let mut values = values;
    let mut diffs = Vec::with_capacity(page);
    for &anchor in &anchors {
        diffs.clear();
        diffs.extend(values.by_ref().take(page).map(|v| to_diff(v, anchor)));
        // Pages always hold `page` values; the last one is padded with 0.
        diffs.resize(page, 0);
        write_packed(out, diffs.iter().copied())?;
    }
    Ok(())
}

/// Writes a `PackedDeque` starting at the front of its buffer.
fn write_deque<W: Write>(out: &mut W, values: impl Iterator<Item = u64> + Clone) -> PanResult<()> {
    let len = values.clone().count();
    out.write_all(&0_u64.to_le_bytes())?;
    out.write_all(&(len as u64).to_le_bytes())?;
    write_packed(out, values)
}

/// Writes a `RobustPagedVector`.
fn write_robust<W: Write>(out: &mut W, values: impl Iterator<Item = u64> + Clone) -> PanResult<()> {
    write_packed(out, values.clone().take(NARROW_PAGE))?;
    write_paged(out, NARROW_PAGE, values.skip(NARROW_PAGE))
}

const fn encode_base(base: u8) -> u64 {
    match base {
        b'A' | b'a' => 0,
        b'C' | b'c' => 1,
        b'G' | b'g' => 2,
        b'T' | b't' => 3,
        _ => 4,
    }
}

/// For every path step in order, the previous membership (1-based index of
/// an earlier step) of the same node, or 0: libbdsg prepends each new
/// membership to its node's list.
#[derive(Clone)]
struct MembershipNext<'a> {
    paths: &'a [Path],
    path: usize,
    step: usize,
    index: u64,
    last: Vec<u64>,
}

impl Iterator for MembershipNext<'_> {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        loop {
            let path = self.paths.get(self.path)?;
            if let Some(step) = path.steps.get(self.step) {
                self.step += 1;
                self.index += 1;
                let last = &mut self.last[step.node_id()];
                return Some(std::mem::replace(last, self.index));
            }
            self.path += 1;
            self.step = 0;
        }
    }
}

/// Edge lists as libbdsg's `create_edge` builds them.
struct EdgeLists {
    /// Per record: start-side and end-side list heads.
    heads: Vec<u64>,
    /// Per edge record: target handle and next record.
    records: Vec<u64>,
    reversing_self_edges: u64,
}

impl EdgeLists {
    fn new(graph: &CoreGraphDTO, handle: &impl Fn(NodeId, bool) -> u64) -> Self {
        let mut heads = vec![0; 2 * graph.nodes.len()];
        let mut records = Vec::with_capacity(4 * graph.edges.len());
        let mut reversing_self_edges = 0;
        // An edge and its reverse complement are the same PackedGraph edge.
        let mut seen = FxHashSet::default();
        for edge in &graph.edges {
            let from_rev = edge.from_orient == Orientation::Reverse;
            let to_rev = edge.to_orient == Orientation::Reverse;
            let left = handle(edge.from_node, from_rev);
            let right = handle(edge.to_node, to_rev);
            if !seen.insert((left, right).min((right ^ 1, left ^ 1))) {
                continue;
            }
            let left_side = 2 * edge.from_node + usize::from(!from_rev);
            let right_side = 2 * edge.to_node + usize::from(to_rev);
            records.extend([right, heads[left_side]]);
            heads[left_side] = (records.len() / 2) as u64;
            if left_side == right_side {
                reversing_self_edges += 1;
                continue;
            }
            records.extend([left ^ 1, heads[right_side]]);
            heads[right_side] = (records.len() / 2) as u64;
        }
        Self {
            heads,
            records,
            reversing_self_edges,
        }
    }
}

/// Writes the path memberships of the nodes, as appending the paths' steps
/// in order records them.
fn write_memberships<W: Write>(out: &mut W, node_count: usize, paths: &[Path]) -> PanResult<()> {
    let mut last_membership = vec![0_u64; node_count];
    let mut index = 0;
    for step in paths.iter().flat_map(|path| &path.steps) {
        index += 1;
        last_membership[step.node_id()] = index;
    }
    write_paged(out, NARROW_PAGE, last_membership.iter().copied())?;
    let membership_path = paths
        .iter()
        .enumerate()
        .flat_map(|(i, path)| std::iter::repeat_n(i as u64, path.steps.len()));
    write_paged(out, WIDE_PAGE, membership_path)?;
    let membership_step = paths.iter().flat_map(|path| 1..=path.steps.len() as u64);
    write_paged(out, NARROW_PAGE, membership_step)?;
    let membership_next = MembershipNext {
        paths,
        path: 0,
        step: 0,
        index: 0,
        last: vec![0; node_count],
    };
    write_paged(out, NARROW_PAGE, membership_next)?;
    Ok(())
}

/// Writes the path names, path metadata and step lists.
fn write_paths<W: Write>(
    out: &mut W,
    paths: &[Path],
    handle: &(impl Fn(NodeId, bool) -> u64 + Sync),
) -> PanResult<()> {
    // Path names, as indices into the bytes in order of first use.
    let mut code = [u64::MAX; 256];
    let mut alphabet = Vec::new();
    for &byte in paths.iter().flat_map(|path| &path.name) {
        if code[usize::from(byte)] == u64::MAX {
            code[usize::from(byte)] = alphabet.len() as u64;
            alphabet.push(byte);
        }
    }
    out.write_all(&(alphabet.len() as u64).to_le_bytes())?;
    out.write_all(&alphabet)?;
    write_packed(
        out,
        paths
            .iter()
            .flat_map(|path| path.name.iter().map(|&b| code[usize::from(b)])),
    )?;
    let name_starts = paths.iter().scan(0_u64, |next, path| {
        let start = *next;
        *next += path.name.len() as u64;
        Some(start)
    });
    write_paged(out, NARROW_PAGE, name_starts)?;
    write_packed(out, paths.iter().map(|path| path.name.len() as u64))?;
    let zeros = std::iter::repeat_n(0, paths.len());
    write_packed(out, zeros.clone())?; // deleted
    write_packed(out, zeros.clone())?; // circular
    let heads = paths.iter().map(|path| u64::from(!path.steps.is_empty()));
    write_paged(out, WIDE_PAGE, heads)?;
    write_paged(
        out,
        NARROW_PAGE,
        paths.iter().map(|path| path.steps.len() as u64),
    )?;
    write_packed(out, zeros)?; // deleted steps

    // The step lists, encoded in parallel a window of paths at a time.
    let window = rayon::current_num_threads().max(1);
    for group in paths.chunks(window) {
        let encoded = group
            .par_iter()
            .map(|path| {
                let mut buf = Vec::new();
                let len = path.steps.len() as u64;
                let links = (0..len).flat_map(|k| [k, if k + 1 < len { k + 2 } else { 0 }]);
                write_robust(&mut buf, links)?;
                let steps = path
                    .steps
                    .iter()
                    .map(|step| handle(step.node_id(), step.is_reverse()));
                write_robust(&mut buf, steps)?;
                Ok(buf)
            })
            .collect::<PanResult<Vec<Vec<u8>>>>()?;
        for buf in encoded {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Writes `graph` (node `v` is record `v + 1`) after the magic number.
fn write_graph<W: Write>(
    out: &mut W,
    graph: &CoreGraphDTO,
    numbering: &Numbering,
) -> PanResult<()> {
    let nodes = graph.nodes.as_slice();
    let paths = graph.paths.as_slice();
    let handle = |node: NodeId, reverse: bool| numbering.id(node) << 1 | u64::from(reverse);

    // An empty graph keeps libbdsg's initial bounds.
    let (min_id, max_id) = (0..numbering.records)
        .find(|&rank| numbering.node_at(rank).is_some())
        .map_or((i64::MAX.cast_unsigned(), 0), |first| {
            (first as u64 + 1, numbering.records as u64)
        });
    out.write_all(&max_id.to_le_bytes())?;
    out.write_all(&min_id.to_le_bytes())?;

    let edges = EdgeLists::new(graph, &handle);
    write_paged(out, NARROW_PAGE, edges.heads.iter().copied())?;
    let starts = nodes.iter().scan(0_u64, |next, node| {
        let start = *next;
        *next += node.sequence.len() as u64;
        Some(start)
    });
    write_paged(out, NARROW_PAGE, starts)?;
    write_packed(out, nodes.iter().map(|node| node.sequence.len() as u64))?;
    write_paged(out, WIDE_PAGE, edges.records.iter().copied())?;
    let first_rank = usize::try_from(min_id.saturating_sub(1)).unwrap_or(usize::MAX);
    write_deque(
        out,
        (first_rank..numbering.records)
            .map(|rank| numbering.node_at(rank).map_or(0, |node| node as u64 + 1)),
    )?;
    let bases = nodes
        .iter()
        .flat_map(|node| node.sequence.iter().map(|&b| encode_base(b)));
    write_packed(out, bases)?;

    write_memberships(out, nodes.len(), paths)?;
    write_paths(out, paths, &handle)?;

    for counter in [0, 0, 0, 0, edges.reversing_self_edges, 0] {
        out.write_all(&u64::to_le_bytes(counter))?;
    }
    Ok(())
}

impl GraphSerializer for PackedGraphCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        if let Some(message) = graph.dangling_reference() {
            return Err(PanGraphXError::Serialize(message));
        }
        let lossy = graph
            .nodes
            .par_iter()
            .map(|node| {
                node.sequence
                    .iter()
                    .filter(|&&b| encode_base(b) == 4 && !b.eq_ignore_ascii_case(&b'N'))
                    .count()
            })
            .sum::<usize>();
        if lossy > 0 {
            warn!("{FORMAT} stores only ACGTN; {lossy} other bases are written as N");
        }
        let numbering = Numbering::new(graph, FORMAT);
        let mut out = BufWriter::with_capacity(1 << 20, writer);
        out.write_all(&PACKED_GRAPH_MAGIC.to_be_bytes())?;
        write_graph(&mut out, graph, &numbering)?;
        out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::core_types::Orientation::{Forward, Reverse};
    use std::io::Cursor;

    fn edge(from: NodeId, from_orient: Orientation, to: NodeId, to_orient: Orientation) -> Edge {
        Edge {
            from_node: from,
            from_orient,
            to_node: to,
            to_orient,
            overlap: 0,
        }
    }

    fn path(name: &str, steps: &[(NodeId, Orientation)]) -> Path {
        Path {
            name: name.as_bytes().to_vec(),
            steps: steps
                .iter()
                .map(|&(node, orientation)| Step::new(node, orientation).unwrap())
                .collect(),
            overlaps: Vec::new(),
        }
    }

    fn to_bytes(graph: &CoreGraphDTO) -> Vec<u8> {
        let mut buf = Vec::new();
        PackedGraphCodec.serialize(graph, &mut buf).unwrap();
        buf
    }

    fn roundtrip(graph: &CoreGraphDTO) -> CoreGraphDTO {
        read_packed_graph(&mut Cursor::new(to_bytes(graph))).unwrap()
    }

    /// Names nodes `1..=n`, as the reader does for such a graph.
    fn one_based(mut graph: CoreGraphDTO) -> CoreGraphDTO {
        graph.node_name_map = Some(
            (0..graph.nodes.len())
                .map(|i| (i, (i + 1).to_string().into_bytes()))
                .collect(),
        );
        graph
    }

    fn sorted_edges(graph: &CoreGraphDTO) -> Vec<(NodeId, bool, NodeId, bool)> {
        let mut edges: Vec<_> = graph
            .edges
            .iter()
            .map(|e| {
                let (a, ar, b, br) = (
                    e.from_node,
                    e.from_orient == Reverse,
                    e.to_node,
                    e.to_orient == Reverse,
                );
                (a, ar, b, br).min((b, !br, a, !ar))
            })
            .collect();
        edges.sort_unstable();
        edges
    }

    #[test]
    fn diff_encoding_roundtrips() {
        for anchor in [0, 1, 7, 1000, 1 << 40] {
            for value in [0, 1, 2, 3, 4, 5, 6, 999, 1000, 1001, 1005, 123_456] {
                assert_eq!(from_diff(to_diff(value, anchor), anchor), value);
            }
        }
        // The documented order of small differences.
        let codes: Vec<u64> = [100, 101, 102, 103, 99, 104]
            .iter()
            .map(|&v| to_diff(v, 100))
            .collect();
        assert_eq!(codes, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn int_vectors_span_words() {
        for width in [1, 3, 7, 13, 33, 63, 64] {
            let mask = u64::MAX >> (64 - width);
            let values: Vec<u64> = (0..300_u64)
                .map(|i| i.wrapping_mul(0x9E37_79B9_7F4A_7C15) & mask)
                .collect();
            let mut buf = Vec::new();
            write_int_vector(&mut buf, values.len(), width, values.iter().copied()).unwrap();
            let read = IntVector::read(&mut Cursor::new(buf)).unwrap();
            let got: Vec<u64> = (0..read.len).map(|i| read.get(i)).collect();
            assert_eq!(got, values, "width {width}");
        }
    }

    #[test]
    fn paged_and_robust_vectors_roundtrip() {
        let values: Vec<u64> = (0..2500_u64)
            .map(|i| if i % 7 == 0 { 0 } else { 5000 - i * 2 })
            .collect();
        let mut buf = Vec::new();
        write_paged(&mut buf, WIDE_PAGE, values.iter().copied()).unwrap();
        write_robust(&mut buf, values.iter().copied()).unwrap();
        write_deque(&mut buf, values.iter().copied()).unwrap();
        let mut cursor = Cursor::new(buf);
        let paged = PagedVector::read(&mut cursor, WIDE_PAGE).unwrap();
        let robust = RobustPagedVector::read(&mut cursor).unwrap();
        let deque = PackedDeque::read(&mut cursor).unwrap();
        for (i, &value) in values.iter().enumerate() {
            assert_eq!(paged.get(i).unwrap(), value);
            assert_eq!(robust.get(i).unwrap(), value);
            assert_eq!(deque.get(i).unwrap(), value);
        }
        assert_eq!(robust.len(), values.len());
        assert!(paged.get(values.len()).is_err());
        assert!(PagedVector::read(&mut Cursor::new(cursor.into_inner()), NARROW_PAGE).is_err());
    }

    #[test]
    fn canonical_copy_keeps_one_of_each_edge() {
        let (a, b) = (2_u64 << 1, 5_u64 << 1);
        // a+ -> b+ is listed on a's end as b+ and on b's start as a-.
        assert_ne!(is_canonical_copy(a, b), is_canonical_copy(b | 1, a | 1));
        // a+ -> a+ (listed twice) and a+ -> a- (listed once).
        assert_ne!(is_canonical_copy(a, a), is_canonical_copy(a | 1, a | 1));
        assert!(is_canonical_copy(a, a | 1));
        assert!(is_canonical_copy(a | 1, a));
    }

    #[test]
    fn roundtrip_keeps_topology_and_paths() {
        let graph = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGAN".to_vec(), b"T".to_vec()]),
            edges: vec![
                edge(0, Forward, 1, Reverse),
                edge(1, Reverse, 2, Forward),
                edge(2, Forward, 2, Forward),
                edge(0, Forward, 0, Reverse),
                edge(2, Reverse, 2, Forward),
                // The reverse complement of the first edge is not repeated.
                edge(1, Forward, 0, Reverse),
            ],
            paths: vec![
                path(
                    "x#1#chr",
                    &[(0, Forward), (1, Reverse), (2, Forward), (2, Forward)],
                ),
                path("empty", &[]),
                path("y", &[(1, Forward)]),
            ],
            node_name_map: None,
        });
        let parsed = roundtrip(&graph);
        assert_eq!(parsed.nodes, graph.nodes);
        assert_eq!(parsed.paths, graph.paths);
        assert_eq!(parsed.node_name_map, graph.node_name_map);
        let mut expected = sorted_edges(&graph);
        expected.dedup();
        assert_eq!(sorted_edges(&parsed), expected);
    }

    #[test]
    fn long_paths_and_many_edges_use_several_pages() {
        let n = 3000;
        let graph = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq((0..n).map(|i| vec![b"ACGT"[i % 4]; 1 + i % 5]).collect()),
            edges: (1..n).map(|i| edge(i - 1, Forward, i, Forward)).collect(),
            paths: vec![
                path("fwd", &(0..n).map(|i| (i, Forward)).collect::<Vec<_>>()),
                path(
                    "rev",
                    &(0..n).rev().map(|i| (i, Reverse)).collect::<Vec<_>>(),
                ),
            ],
            node_name_map: None,
        });
        let parsed = roundtrip(&graph);
        assert_eq!(parsed.nodes, graph.nodes);
        assert_eq!(parsed.paths, graph.paths);
        assert_eq!(sorted_edges(&parsed), sorted_edges(&graph));
    }

    #[test]
    fn non_acgtn_bases_become_n() {
        let graph = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"acgtRYn".to_vec()]),
            ..CoreGraphDTO::default()
        };
        assert_eq!(roundtrip(&graph).nodes[0].sequence, b"ACGTNNN");
    }

    #[test]
    fn numeric_names_are_kept_with_gaps() {
        let mut graph = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec(), b"G".to_vec()]),
            edges: vec![edge(0, Forward, 2, Forward)],
            paths: vec![path("p", &[(2, Reverse), (0, Reverse)])],
            node_name_map: None,
        };
        graph.node_name_map = Some(HashMap::from([
            (0, b"7".to_vec()),
            (1, b"3".to_vec()),
            (2, b"10".to_vec()),
        ]));
        let parsed = roundtrip(&graph);
        // Read back in id order: 3, 7, 10.
        let names: Vec<Vec<u8>> = (0..3)
            .map(|i| parsed.node_name_map.as_ref().unwrap()[&i].clone())
            .collect();
        assert_eq!(names, [b"3".to_vec(), b"7".to_vec(), b"10".to_vec()]);
        assert_eq!(parsed.nodes[1].sequence, b"A");
        assert_eq!(sorted_edges(&parsed), [(1, false, 2, false)]);
        let steps: Vec<_> = parsed.paths[0]
            .steps
            .iter()
            .map(|s| (s.node_id(), s.is_reverse()))
            .collect();
        assert_eq!(steps, [(2, true), (1, true)]);
    }

    #[test]
    fn non_numeric_names_are_replaced() {
        let mut graph = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
            ..CoreGraphDTO::default()
        };
        graph.node_name_map = Some(HashMap::from([(0, b"x".to_vec()), (1, b"2".to_vec())]));
        let parsed = roundtrip(&graph);
        assert_eq!(parsed.node_name_map, one_based(graph).node_name_map);
    }

    /// A file as vg leaves it after edits: node id 2 deleted (its record
    /// stays), an unlinked edge record and path step, a deleted path, and a
    /// circular path. Live graph: `1: AC`, `3: T`, edge `1+ -> 3+`, paths
    /// `q = 1+ 3-` and circular `c = 3+`.
    fn edited_file() -> Vec<u8> {
        fn paged(buf: &mut Vec<u8>, page: usize, values: &[u64]) {
            write_paged(buf, page, values.iter().copied()).unwrap();
        }
        fn packed(buf: &mut Vec<u8>, values: &[u64]) {
            write_packed(buf, values.iter().copied()).unwrap();
        }
        let mut buf = PACKED_GRAPH_MAGIC.to_be_bytes().to_vec();
        buf.extend(3_u64.to_le_bytes()); // max id
        buf.extend(1_u64.to_le_bytes()); // min id
        paged(&mut buf, NARROW_PAGE, &[0, 2, 0, 0, 3, 0]); // edge list heads
        paged(&mut buf, NARROW_PAGE, &[0, 2, 4]); // sequence starts
        packed(&mut buf, &[2, 2, 1]); // sequence lengths
        // Edge record 1 (to node 2) is unlinked; 2 and 3 store 1+ -> 3+.
        paged(&mut buf, WIDE_PAGE, &[2 << 1, 0, 3 << 1, 0, 1 << 1 | 1, 0]);
        write_deque(&mut buf, [1, 0, 3].into_iter()).unwrap();
        packed(&mut buf, &[0, 1, 2, 2, 3]); // AC GG T
        for page in [NARROW_PAGE, WIDE_PAGE, NARROW_PAGE, NARROW_PAGE] {
            paged(&mut buf, page, &[]); // memberships, not read
        }
        buf.extend(3_u64.to_le_bytes());
        buf.extend(b"pqc");
        packed(&mut buf, &[0, 1, 2]);
        paged(&mut buf, NARROW_PAGE, &[0, 1, 2]); // name starts
        packed(&mut buf, &[1, 1, 1]); // name lengths
        packed(&mut buf, &[1, 0, 0]); // deleted
        packed(&mut buf, &[0, 0, 1]); // circular
        paged(&mut buf, WIDE_PAGE, &[0, 2, 1]); // heads
        paged(&mut buf, NARROW_PAGE, &[0, 3, 1]); // tails
        packed(&mut buf, &[0, 1, 0]); // deleted steps
        let mut path = |links: &[u64], steps: &[u64]| {
            write_robust(&mut buf, links.iter().copied()).unwrap();
            write_robust(&mut buf, steps.iter().copied()).unwrap();
        };
        path(&[], &[]);
        // Step 1 (on the deleted node) is unlinked.
        path(&[0, 0, 0, 3, 2, 0], &[2 << 1, 1 << 1, 3 << 1 | 1]);
        path(&[1, 1], &[3 << 1]);
        for counter in [1, 1, 1, 2, 0, 0] {
            buf.extend(u64::to_le_bytes(counter));
        }
        buf
    }

    #[test]
    fn reads_files_with_deleted_records() {
        let parsed = read_packed_graph(&mut Cursor::new(edited_file())).unwrap();
        assert_eq!(
            parsed.nodes,
            Nodes::from_seq(vec![b"AC".to_vec(), b"T".to_vec()])
        );
        assert_eq!(
            parsed.node_name_map,
            Some(HashMap::from([(0, b"1".to_vec()), (1, b"3".to_vec())]))
        );
        assert_eq!(sorted_edges(&parsed), [(0, false, 1, false)]);
        assert_eq!(
            parsed.paths,
            [
                path("q", &[(0, Forward), (1, Reverse)]),
                path("c", &[(1, Forward)])
            ]
        );
    }

    #[test]
    fn empty_graph() {
        let parsed = roundtrip(&CoreGraphDTO::default());
        assert_eq!(
            (parsed.nodes.len(), parsed.edges.len(), parsed.paths.len()),
            (0, 0, 0)
        );
    }

    #[test]
    fn rejects_truncated_and_corrupt_input() {
        let graph = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![path("p", &[(0, Forward), (1, Reverse)])],
            node_name_map: None,
        });
        let buf = to_bytes(&graph);
        for len in [0, 3, 20, buf.len() / 2, buf.len() - 1] {
            assert!(
                read_packed_graph(&mut Cursor::new(&buf[..len])).is_err(),
                "{len}"
            );
        }
        let mut bad = buf;
        bad[0] ^= 1;
        assert!(read_packed_graph(&mut Cursor::new(bad)).is_err());
        let mut garbage = PACKED_GRAPH_MAGIC.to_be_bytes().to_vec();
        garbage.extend([0xFF; 256]);
        assert!(read_packed_graph(&mut Cursor::new(garbage)).is_err());
    }
}
