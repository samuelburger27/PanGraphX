//! Native reader and writer for odgi's binary graph format (`.og`).
//!
//! The layout mirrors odgi's `graph_t::serialize` (libhandlegraph `Serializable`
//! with odgi's `node_t` records and DYNAMIC's `hacked_vector`). All integers are
//! little-endian except the leading magic number:
//!
//! ```text
//! u32 BE  magic (1988148666)
//! u64 ×7  max_node_id, min_node_id, node_count, edge_count, path_count,
//!         path_handle_next, id_increment
//! node_count × node record (node rank r has id r + 1 + id_increment):
//!   u64 sequence length, sequence bytes, u64 id (0 marks a deleted node)
//!   packed edges    (other node id, other_rev | on_rev << 1 | to_curr << 2) pairs
//!   packed decoding deltas to neighbor ids (0 = this node, odd = higher, even = lower)
//!   packed steps    (path, rev | start << 1 | end << 2 | del << 3,
//!                    prev, prev rank, next, next rank) where prev/next index decoding
//! path_count × path: u64 length, first step (u64 handle, u64 rank),
//!                    last step (u64 handle, u64 rank), u64 name length, name bytes
//! packed = u64 word count, words, u64 mask, u64 length, u8 width, u8 values per word
//! ```
//!
//! Edges are stored on both of their nodes (`to_curr` on the right one, self loops
//! once). A path is a linked list of steps; a step is named by its node handle
//! (`rank << 1 | is_reverse`) and its rank among the steps stored on that node.
//!
//! `docs/odgi_format.md` describes every field in detail, with a worked example
//! decoded from `odgi build` output; `utils/dump_odgi.py` prints a file's fields.

use crate::core::core_types::{Edge, NodeId, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::binio::{self, MAX_PREALLOC};
use crate::formats::id_map::Numbering;
use crate::traits::{GraphParser, GraphSerializer};
use log::warn;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, Write};

/// Codec for odgi's binary graph format.
///
/// Node ids are written as odgi ids: numeric node names are kept when they are
/// distinct positive integers that fit a dense id range (as `odgi build` keeps
/// GFA segment ids), otherwise nodes are numbered `1..=n`. Parsed nodes are named
/// by their odgi id. Overlaps are not part of the format and are dropped.
pub struct ODGICodec;

/// libhandlegraph magic number of odgi's `graph_t`.
const ODGI_MAGIC: u32 = 1_988_148_666;

/// Values per edge record and per step record in a node.
const EDGE_RECORD_LEN: usize = 2;
const STEP_RECORD_LEN: usize = 6;

/// Bits of a packed edge.
const EDGE_OTHER_REV: u64 = 1;
const EDGE_ON_REV: u64 = 1 << 1;
const EDGE_TO_CURR: u64 = 1 << 2;

/// Bits of a step type.
const STEP_REV: u64 = 1;
const STEP_START: u64 = 1 << 1;
const STEP_END: u64 = 1 << 2;

/// Marks a deleted node rank.
const VACANT: NodeId = NodeId::MAX;

/// Target amount of output encoded per parallel task.
const CHUNK_BYTES: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Format name used in error messages.
const FORMAT: &str = "ODGI";

fn read_u64<R: Read>(reader: &mut R) -> PanResult<u64> {
    Ok(u64::from_le_bytes(binio::read_array(reader, FORMAT)?))
}

fn read_u8<R: Read>(reader: &mut R) -> PanResult<u8> {
    Ok(binio::read_array::<1, _>(reader, FORMAT)?[0])
}

fn to_usize(value: u64, what: &str) -> PanResult<usize> {
    binio::to_usize(value, FORMAT, what)
}

fn read_bytes<R: Read>(reader: &mut R, len: u64) -> PanResult<Vec<u8>> {
    binio::read_bytes(reader, len, FORMAT)
}

/// A DYNAMIC `hacked_vector`: fixed-width integers packed into 64-bit words.
struct PackedInts {
    words: Vec<u64>,
    len: usize,
    width: usize,
    per_word: usize,
    mask: u64,
}

impl PackedInts {
    fn read<R: Read>(reader: &mut R) -> PanResult<Self> {
        let word_count = to_usize(read_u64(reader)?, "word count")?;
        let mut words = Vec::with_capacity(word_count.min(MAX_PREALLOC));
        for _ in 0..word_count {
            words.push(read_u64(reader)?);
        }
        let _mask = read_u64(reader)?;
        let len = to_usize(read_u64(reader)?, "vector length")?;
        let width = usize::from(read_u8(reader)?);
        let _per_word = read_u8(reader)?;
        if len == 0 {
            return Ok(Self {
                words,
                len,
                width: 64,
                per_word: 1,
                mask: u64::MAX,
            });
        }
        if width == 0 || width > 64 {
            return Err(PanGraphXError::Parse(format!(
                "Invalid ODGI packed vector width {width}"
            )));
        }
        let per_word = 64 / width;
        if words.len() < len.div_ceil(per_word) {
            return Err(PanGraphXError::Parse(
                "ODGI packed vector is shorter than its length".to_string(),
            ));
        }
        let mask = if width == 64 {
            u64::MAX
        } else {
            (1 << width) - 1
        };
        Ok(Self {
            words,
            len,
            width,
            per_word,
            mask,
        })
    }

    /// Value at `index`; `index` must be below `len`.
    fn get(&self, index: usize) -> u64 {
        let word = self.words[index / self.per_word];
        (word >> ((index % self.per_word) * self.width)) & self.mask
    }
}

/// The part of a stored step needed to walk a path.
#[derive(Clone, Copy)]
struct StepLink {
    /// Rank of the node holding the next step.
    next_node: u32,
    /// Rank of the next step among the steps of that node.
    next_rank: u32,
    is_reverse: bool,
    is_end: bool,
}

/// A path's metadata record.
struct PathMeta {
    name: Vec<u8>,
    length: u64,
    first_handle: u64,
    first_rank: u64,
}

/// Steps of all nodes, indexed by node rank, used to walk the paths.
struct StepTable {
    /// Steps of node rank `r` are `links[offsets[r]..offsets[r + 1]]`.
    offsets: Vec<usize>,
    links: Vec<StepLink>,
    rank_to_node: Vec<NodeId>,
}

impl StepTable {
    fn walk(&self, meta: PathMeta) -> PanResult<Path> {
        let name = String::from_utf8_lossy(&meta.name).into_owned();
        let broken = |what: &str| PanGraphXError::Parse(format!("ODGI path {name} {what}"));
        let length = to_usize(meta.length, "path length")?;
        if length > self.links.len() {
            return Err(broken("is longer than the number of stored steps"));
        }
        let mut steps = Vec::with_capacity(length);
        let mut node = to_usize(meta.first_handle >> 1, "node rank")?;
        let mut rank = to_usize(meta.first_rank, "step rank")?;
        for index in 0..length {
            let (Some(&start), Some(&end)) = (self.offsets.get(node), self.offsets.get(node + 1))
            else {
                return Err(broken("refers to a missing node"));
            };
            if rank >= end - start {
                return Err(broken("refers to a missing step"));
            }
            let link = self.links[start + rank];
            steps.push(Step::new(
                self.rank_to_node[node],
                Orientation::from(link.is_reverse),
            )?);
            if index + 1 < length {
                if link.is_end {
                    return Err(broken("ends before its length"));
                }
                node = link.next_node as usize;
                rank = link.next_rank as usize;
            }
        }
        Ok(Path {
            name: meta.name,
            steps,
            overlaps: Vec::new(),
        })
    }
}

#[allow(clippy::too_many_lines)]
fn read_graph<R: Read>(reader: &mut R) -> PanResult<CoreGraphDTO> {
    let _max_node_id = read_u64(reader)?;
    let _min_node_id = read_u64(reader)?;
    let record_count = to_usize(read_u64(reader)?, "node count")?;
    let edge_count = to_usize(read_u64(reader)?, "edge count")?;
    let path_count = to_usize(read_u64(reader)?, "path count")?;
    let _path_handle_next = read_u64(reader)?;
    let id_increment = read_u64(reader)?;
    if u32::try_from(record_count).is_err() {
        return Err(PanGraphXError::Parse(format!(
            "ODGI node count {record_count} is too large"
        )));
    }
    // Maps an odgi node id to its node rank.
    let id_to_rank = |id: u64| {
        id.checked_sub(id_increment)
            .and_then(|id| id.checked_sub(1))
            .filter(|&rank| rank < record_count as u64)
            .ok_or_else(|| PanGraphXError::Parse(format!("ODGI node id {id} is out of range")))
    };

    let mut nodes = Nodes::new();
    let mut node_name_map: HashMap<NodeId, Vec<u8>> =
        HashMap::with_capacity(record_count.min(MAX_PREALLOC));
    let mut rank_to_node = Vec::with_capacity(record_count.min(MAX_PREALLOC));
    // `to_node` holds the node rank until all nodes are read.
    let mut edges: Vec<Edge> = Vec::with_capacity(edge_count.min(MAX_PREALLOC));
    let mut offsets = Vec::with_capacity(record_count.min(MAX_PREALLOC) + 1);
    offsets.push(0);
    let mut links: Vec<StepLink> = Vec::new();
    let mut id_buf = itoa::Buffer::new();

    for rank in 0..record_count {
        let sequence_len = read_u64(reader)?;
        let sequence = read_bytes(reader, sequence_len)?;
        let stored_id = read_u64(reader)?;
        let edge_values = PackedInts::read(reader)?;
        let decoding = PackedInts::read(reader)?;
        let step_values = PackedInts::read(reader)?;

        if stored_id == 0 {
            // Deleted nodes are stored as empty records.
            rank_to_node.push(VACANT);
            offsets.push(links.len());
            continue;
        }
        let node_id = nodes.len();
        rank_to_node.push(node_id);
        nodes.push(sequence);
        let odgi_id = rank as u64 + 1 + id_increment;
        node_name_map.insert(node_id, id_buf.format(odgi_id).as_bytes().to_vec());

        if edge_values.len % EDGE_RECORD_LEN != 0 {
            return Err(PanGraphXError::Parse(format!(
                "ODGI node {odgi_id} has an incomplete edge record"
            )));
        }
        for record in (0..edge_values.len).step_by(EDGE_RECORD_LEN) {
            let packed = edge_values.get(record + 1);
            // The edge is read from its left node; `to_curr` marks the right node's copy.
            if packed & EDGE_TO_CURR != 0 {
                continue;
            }
            edges.push(Edge {
                from_node: node_id,
                from_orient: Orientation::from(packed & EDGE_ON_REV != 0),
                to_node: to_usize(id_to_rank(edge_values.get(record))?, "node rank")?,
                to_orient: Orientation::from(packed & EDGE_OTHER_REV != 0),
                overlap: 0,
            });
        }

        if step_values.len % STEP_RECORD_LEN != 0 {
            return Err(PanGraphXError::Parse(format!(
                "ODGI node {odgi_id} has an incomplete step record"
            )));
        }
        for record in (0..step_values.len).step_by(STEP_RECORD_LEN) {
            let step_type = step_values.get(record + 1);
            let is_end = step_type & STEP_END != 0;
            let (next_node, next_rank) = if is_end {
                (0, 0)
            } else {
                let index = to_usize(step_values.get(record + 4), "decoding index")?;
                if index >= decoding.len {
                    return Err(PanGraphXError::Parse(format!(
                        "ODGI node {odgi_id} has a step with an invalid neighbor"
                    )));
                }
                let delta = decoding.get(index);
                let next_id = match delta {
                    0 => Some(stored_id),
                    d if d & 1 == 1 => stored_id.checked_add(d >> 1),
                    d => stored_id.checked_sub(d >> 1),
                }
                .ok_or_else(|| {
                    PanGraphXError::Parse(format!("ODGI node {odgi_id} has an invalid neighbor"))
                })?;
                let next_rank = u32::try_from(step_values.get(record + 5)).map_err(|_| {
                    PanGraphXError::Parse(format!("ODGI node {odgi_id} has an invalid step rank"))
                })?;
                // `record_count` fits in `u32`, so every rank does.
                #[allow(clippy::cast_possible_truncation)]
                let next_node = id_to_rank(next_id)? as u32;
                (next_node, next_rank)
            };
            links.push(StepLink {
                next_node,
                next_rank,
                is_reverse: step_type & STEP_REV != 0,
                is_end,
            });
        }
        offsets.push(links.len());
    }

    for edge in &mut edges {
        edge.to_node = match rank_to_node[edge.to_node] {
            VACANT => {
                return Err(PanGraphXError::Parse(
                    "ODGI edge refers to a deleted node".to_string(),
                ));
            }
            node => node,
        };
    }

    let mut metas = Vec::with_capacity(path_count.min(MAX_PREALLOC));
    for _ in 0..path_count {
        let length = read_u64(reader)?;
        let first_handle = read_u64(reader)?;
        let first_rank = read_u64(reader)?;
        let _last_handle = read_u64(reader)?;
        let _last_rank = read_u64(reader)?;
        let name_len = read_u64(reader)?;
        let name = read_bytes(reader, name_len)?;
        metas.push(PathMeta {
            name,
            length,
            first_handle,
            first_rank,
        });
    }

    let table = StepTable {
        offsets,
        links,
        rank_to_node,
    };
    let paths = metas
        .into_par_iter()
        .map(|meta| table.walk(meta))
        .collect::<PanResult<Vec<Path>>>()?;

    Ok(CoreGraphDTO {
        nodes,
        edges,
        paths,
        node_name_map: Some(node_name_map),
    })
}

impl<R: Read + Seek> GraphParser<R> for ODGICodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        let mut reader = BufReader::with_capacity(1 << 20, reader);
        let mut magic = [0u8; 4];
        reader
            .read_exact(&mut magic)
            .map_err(|e| binio::truncated(FORMAT, e))?;
        if u32::from_be_bytes(magic) != ODGI_MAGIC {
            // odgi accepts files without the magic number (older versions), so do we.
            warn!("ODGI magic number not found; reading the file anyway");
            reader.seek_relative(-4)?;
        }
        read_graph(&mut reader)
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Appends `values` as a `hacked_vector` of the smallest width that fits them.
#[allow(clippy::cast_possible_truncation)]
fn push_packed(buf: &mut Vec<u8>, values: &[u64]) {
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

/// Edge records of every node, as odgi stores them on both endpoints.
struct EdgeIndex {
    /// Records of node `v` are `records[offsets[v]..offsets[v + 1]]`.
    offsets: Vec<usize>,
    /// Other node and packed edge type.
    records: Vec<(NodeId, u8)>,
    /// Number of distinct edges.
    edge_count: usize,
}

impl EdgeIndex {
    #[allow(clippy::cast_possible_truncation)]
    fn new(node_count: usize, edges: &[Edge]) -> Self {
        // An edge and its reverse complement are the same odgi edge.
        let handle = |node: NodeId, orientation| {
            node << 1 | usize::from(orientation == Orientation::Reverse)
        };
        let mut seen = FxHashSet::default();
        let unique: Vec<&Edge> = edges
            .iter()
            .filter(|edge| {
                let from = handle(edge.from_node, edge.from_orient);
                let to = handle(edge.to_node, edge.to_orient);
                seen.insert((from, to).min((to ^ 1, from ^ 1)))
            })
            .collect();
        drop(seen);

        let mut offsets = vec![0; node_count + 1];
        for edge in &unique {
            offsets[edge.from_node + 1] += 1;
            if edge.to_node != edge.from_node {
                offsets[edge.to_node + 1] += 1;
            }
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut cursor = offsets.clone();
        let mut records = vec![(0, 0); offsets[node_count]];
        for edge in &unique {
            let from_rev = u64::from(edge.from_orient == Orientation::Reverse);
            let to_rev = u64::from(edge.to_orient == Orientation::Reverse);
            records[cursor[edge.from_node]] = (edge.to_node, (to_rev | from_rev << 1) as u8);
            cursor[edge.from_node] += 1;
            if edge.to_node != edge.from_node {
                records[cursor[edge.to_node]] = (
                    edge.from_node,
                    (from_rev | to_rev << 1 | EDGE_TO_CURR) as u8,
                );
                cursor[edge.to_node] += 1;
            }
        }
        Self {
            offsets,
            records,
            edge_count: unique.len(),
        }
    }
}

/// Path steps grouped by the node they visit.
struct StepIndex {
    /// Steps on node `v` are `slots[offsets[v]..offsets[v + 1]]`, ordered by
    /// their rank on that node.
    offsets: Vec<usize>,
    /// Global index (all paths concatenated) of the step in each slot.
    slots: Vec<usize>,
    /// Rank of every global step on its node.
    ranks: Vec<u32>,
    /// Global index of the first step of every path, plus the total.
    path_starts: Vec<usize>,
}

impl StepIndex {
    fn new(node_count: usize, paths: &[Path]) -> PanResult<Self> {
        let mut path_starts = Vec::with_capacity(paths.len() + 1);
        path_starts.push(0);
        let mut offsets = vec![0; node_count + 1];
        for path in paths {
            path_starts.push(path_starts[path_starts.len() - 1] + path.steps.len());
            for step in &path.steps {
                offsets[step.node_id() + 1] += 1;
            }
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let total = offsets[node_count];
        let mut cursor = offsets.clone();
        let mut slots = vec![0; total];
        let mut ranks = vec![0; total];
        for (global, step) in paths.iter().flat_map(|path| &path.steps).enumerate() {
            let node = step.node_id();
            let slot = cursor[node];
            cursor[node] += 1;
            slots[slot] = global;
            ranks[global] = u32::try_from(slot - offsets[node]).map_err(|_| {
                PanGraphXError::Serialize(format!("Node {node} is visited too often for ODGI"))
            })?;
        }
        Ok(Self {
            offsets,
            slots,
            ranks,
            path_starts,
        })
    }

    /// Path index and position within that path of a global step.
    fn locate(&self, global: usize) -> (usize, usize) {
        // Empty paths share their start with the next path; the last path
        // starting at or before `global` is the one containing it.
        let path = self.path_starts.partition_point(|&start| start <= global) - 1;
        (path, global - self.path_starts[path])
    }
}

/// Per-task buffers for encoding node records.
#[derive(Default)]
struct Scratch {
    values: Vec<u64>,
    decoding: Vec<u64>,
    decoding_index: FxHashMap<u64, u64>,
}

impl Scratch {
    /// Index of `other_id` in the node's decoding table, adding it if needed.
    fn encode(&mut self, own_id: u64, other_id: u64) -> u64 {
        let delta = match other_id.cmp(&own_id) {
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => (other_id - own_id) << 1 | 1,
            std::cmp::Ordering::Less => (own_id - other_id) << 1,
        };
        let next = self.decoding.len() as u64;
        *self.decoding_index.entry(delta).or_insert_with(|| {
            self.decoding.push(delta);
            next
        })
    }
}

struct OdgiWriter<'a> {
    graph: &'a CoreGraphDTO,
    numbering: Numbering,
    edges: EdgeIndex,
    steps: StepIndex,
}

impl OdgiWriter<'_> {
    fn header(&self) -> Vec<u8> {
        let ranks = || {
            (0..self.numbering.records)
                .filter_map(|r| self.numbering.node_at(r).map(|_| r as u64 + 1))
        };
        let mut buf = Vec::with_capacity(60);
        buf.extend_from_slice(&ODGI_MAGIC.to_be_bytes());
        push_u64(&mut buf, ranks().next_back().unwrap_or(0));
        push_u64(&mut buf, ranks().next().unwrap_or(0));
        push_u64(&mut buf, self.numbering.records as u64);
        push_u64(&mut buf, self.edges.edge_count as u64);
        push_u64(&mut buf, self.graph.paths.len() as u64);
        push_u64(&mut buf, self.graph.paths.len() as u64);
        push_u64(&mut buf, 0); // id increment
        buf
    }

    /// Appends the record of node rank `rank`.
    fn push_record(&self, rank: usize, buf: &mut Vec<u8>, scratch: &mut Scratch) {
        let Some(node) = self.numbering.node_at(rank) else {
            // A deleted node: empty sequence, id 0 and three empty vectors.
            push_u64(buf, 0);
            push_u64(buf, 0);
            for _ in 0..3 {
                push_packed(buf, &[]);
            }
            return;
        };
        let own_id = rank as u64 + 1;
        let sequence = &self.graph.nodes.as_slice()[node].sequence;
        push_u64(buf, sequence.len() as u64);
        buf.extend_from_slice(sequence);
        push_u64(buf, own_id);

        scratch.values.clear();
        let edges = &self.edges;
        for &(other, packed) in &edges.records[edges.offsets[node]..edges.offsets[node + 1]] {
            scratch.values.push(self.numbering.id(other));
            scratch.values.push(u64::from(packed));
        }
        push_packed(buf, &scratch.values);

        scratch.values.clear();
        scratch.decoding.clear();
        scratch.decoding_index.clear();
        let steps = &self.steps;
        if steps.offsets[node] < steps.offsets[node + 1] {
            // odgi's `create_step` encodes the node itself before linking any step.
            scratch.encode(own_id, own_id);
        }
        for &global in &steps.slots[steps.offsets[node]..steps.offsets[node + 1]] {
            let (path_index, position) = steps.locate(global);
            let path = &self.graph.paths[path_index].steps;
            let is_start = position == 0;
            let is_end = position + 1 == path.len();
            let (prev_id, prev_rank) = if is_start {
                (own_id, 0)
            } else {
                let prev = path[position - 1].node_id();
                (self.numbering.id(prev), steps.ranks[global - 1])
            };
            let (next_id, next_rank) = if is_end {
                (own_id, 0)
            } else {
                let next = path[position + 1].node_id();
                (self.numbering.id(next), steps.ranks[global + 1])
            };
            let step_type = u64::from(path[position].is_reverse())
                | (u64::from(is_start) * STEP_START)
                | (u64::from(is_end) * STEP_END);
            let prev = scratch.encode(own_id, prev_id);
            let next = scratch.encode(own_id, next_id);
            scratch.values.extend_from_slice(&[
                path_index as u64 + 1,
                step_type,
                prev,
                u64::from(prev_rank),
                next,
                u64::from(next_rank),
            ]);
        }
        push_packed(buf, &scratch.decoding);
        push_packed(buf, &scratch.values);
    }

    /// Node ranks per parallel task so that a chunk holds about [`CHUNK_BYTES`].
    fn ranks_per_chunk(&self) -> usize {
        let records = self.numbering.records.max(1);
        let sequence_bytes: usize = self
            .graph
            .nodes
            .par_iter()
            .map(|node| node.sequence.len())
            .sum();
        let average =
            (sequence_bytes + 16 * self.edges.records.len() + 48 * self.steps.slots.len())
                / records
                + 100;
        (CHUNK_BYTES / average).clamp(1, 1 << 14)
    }

    fn write_records(&self, writer: &mut dyn Write) -> PanResult<()> {
        let per_chunk = self.ranks_per_chunk();
        let records = self.numbering.records;
        let window = per_chunk * rayon::current_num_threads();
        for window_start in (0..records).step_by(window) {
            let window_end = (window_start + window).min(records);
            let buffers: Vec<Vec<u8>> = (window_start..window_end)
                .into_par_iter()
                .step_by(per_chunk)
                .map(|start| {
                    let mut buf = Vec::new();
                    let mut scratch = Scratch::default();
                    for rank in start..(start + per_chunk).min(window_end) {
                        self.push_record(rank, &mut buf, &mut scratch);
                    }
                    buf
                })
                .collect();
            for buf in buffers {
                writer.write_all(&buf)?;
            }
        }
        Ok(())
    }

    fn write_paths(&self, writer: &mut dyn Write) -> PanResult<()> {
        let mut buf = Vec::new();
        for (index, path) in self.graph.paths.iter().enumerate() {
            let step_handle = |position: usize| {
                let step = path.steps[position];
                let rank = self.numbering.id(step.node_id()) - 1;
                let global = self.steps.path_starts[index] + position;
                (
                    rank << 1 | u64::from(step.is_reverse()),
                    u64::from(self.steps.ranks[global]),
                )
            };
            let (first, last) = match path.steps.len() {
                0 => ((0, 0), (0, 0)),
                len => (step_handle(0), step_handle(len - 1)),
            };
            push_u64(&mut buf, path.steps.len() as u64);
            for value in [first.0, first.1, last.0, last.1, path.name.len() as u64] {
                push_u64(&mut buf, value);
            }
            buf.extend_from_slice(&path.name);
            if buf.len() >= CHUNK_BYTES {
                writer.write_all(&buf)?;
                buf.clear();
            }
        }
        writer.write_all(&buf)?;
        Ok(())
    }
}

impl GraphSerializer for ODGICodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        if let Some(message) = graph.dangling_reference() {
            return Err(PanGraphXError::Serialize(message));
        }
        if graph.edges.par_iter().any(|edge| edge.overlap != 0)
            || graph
                .paths
                .par_iter()
                .any(|path| path.overlaps.iter().any(|&overlap| overlap != 0))
        {
            warn!("ODGI files don't support overlaps; overlaps are dropped.");
        }
        let node_count = graph.nodes.len();
        let odgi = OdgiWriter {
            graph,
            numbering: Numbering::new(graph, FORMAT),
            edges: EdgeIndex::new(node_count, &graph.edges),
            steps: StepIndex::new(node_count, &graph.paths)?,
        };
        writer.write_all(&odgi.header())?;
        odgi.write_records(writer)?;
        odgi.write_paths(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn roundtrip(graph: &CoreGraphDTO) -> CoreGraphDTO {
        let mut buf = Vec::new();
        ODGICodec.serialize(graph, &mut buf).unwrap();
        ODGICodec.parse(&mut Cursor::new(buf)).unwrap()
    }

    /// Names nodes by their 1-based id, as they come back from odgi.
    fn one_based(mut graph: CoreGraphDTO) -> CoreGraphDTO {
        graph.node_name_map = Some(
            (0..graph.nodes.len())
                .map(|id| (id, (id + 1).to_string().into_bytes()))
                .collect(),
        );
        graph
    }

    use Orientation::{Forward, Reverse};

    #[test]
    fn roundtrip_keeps_orientations() {
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![path("p", &[(0, Forward), (1, Reverse)])],
            node_name_map: None,
        };
        let parsed = roundtrip(&original);
        assert_eq!(parsed.get_name_from_id(0), "1");
        assert_eq!(parsed.get_name_from_id(1), "2");
        assert_eq!(parsed, one_based(original));
    }

    #[test]
    fn roundtrip_self_loops_revisits_and_empty_paths() {
        let original = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"CC".to_vec(), b"GGG".to_vec()]),
            edges: vec![
                edge(0, Forward, 0, Forward),
                edge(1, Forward, 1, Reverse),
                edge(2, Reverse, 0, Forward),
                edge(0, Forward, 1, Forward),
            ],
            paths: vec![
                path(
                    "a",
                    &[(0, Forward), (1, Forward), (0, Forward), (0, Reverse)],
                ),
                path("empty", &[]),
                path(
                    "b",
                    &[(2, Reverse), (0, Forward), (1, Forward), (1, Reverse)],
                ),
            ],
            node_name_map: None,
        });
        // Edges come back grouped by node, so compare them as a set.
        let parsed = roundtrip(&original);
        assert!(parsed.isomorphic(&original));
        assert_eq!(parsed.paths, original.paths);
    }

    #[test]
    fn reverse_complement_edges_are_merged() {
        let graph = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
            edges: vec![
                edge(0, Forward, 1, Forward),
                edge(1, Reverse, 0, Reverse),
                edge(0, Forward, 1, Forward),
            ],
            paths: Vec::new(),
            node_name_map: None,
        };
        assert_eq!(roundtrip(&graph).edges, vec![edge(0, Forward, 1, Forward)]);
    }

    #[test]
    fn numeric_names_are_kept_with_gaps() {
        let names = ["7", "3", "1"];
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec(), b"G".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse), edge(2, Forward, 0, Forward)],
            paths: vec![path("p", &[(2, Forward), (0, Forward), (1, Reverse)])],
            node_name_map: Some(
                names
                    .iter()
                    .enumerate()
                    .map(|(id, name)| (id, name.as_bytes().to_vec()))
                    .collect(),
            ),
        };
        let parsed = roundtrip(&original);
        // Nodes come back in id order; ids 2 and 4..=6 are deleted records.
        assert_eq!(parsed.nodes.len(), 3);
        assert!(parsed.isomorphic(&original));
    }

    #[test]
    fn non_numeric_names_are_replaced() {
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec()]),
            edges: vec![edge(0, Forward, 1, Forward)],
            paths: Vec::new(),
            node_name_map: Some(HashMap::from([(0, b"x".to_vec()), (1, b"01".to_vec())])),
        };
        let parsed = roundtrip(&original);
        assert_eq!(parsed.get_name_from_id(0), "1");
        assert_eq!(parsed.get_name_from_id(1), "2");
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
    fn packed_widths() {
        for values in [
            vec![0],
            vec![1, 0, 1],
            vec![u64::MAX, 3],
            (0..1000).collect(),
        ] {
            let mut buf = Vec::new();
            push_packed(&mut buf, &values);
            let packed = PackedInts::read(&mut Cursor::new(buf)).unwrap();
            let read: Vec<u64> = (0..packed.len).map(|i| packed.get(i)).collect();
            assert_eq!(read, values);
        }
    }

    #[test]
    fn rejects_truncated_and_corrupt_input() {
        let graph = one_based(CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![edge(0, Forward, 1, Reverse)],
            paths: vec![path("p", &[(0, Forward), (1, Reverse)])],
            node_name_map: None,
        });
        let mut buf = Vec::new();
        ODGICodec.serialize(&graph, &mut buf).unwrap();
        for len in [0, 3, 20, buf.len() / 2, buf.len() - 1] {
            assert!(
                ODGICodec.parse(&mut Cursor::new(&buf[..len])).is_err(),
                "{len}"
            );
        }
        assert!(ODGICodec.parse(&mut Cursor::new(vec![0xFF; 256])).is_err());
    }
}
