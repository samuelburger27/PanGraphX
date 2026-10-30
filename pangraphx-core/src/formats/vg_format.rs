use crate::core::core_types::{Edge, Node, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::id_map::{IdMap, parse_canonical_int};
use crate::proto_gen::vg as vg_proto;
use crate::proto_gen::vg_read;
use crate::traits::{GraphParser, GraphSerializer};
use log::debug;
use prost::Message;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::ops::Range;

pub struct VGCodec;

// ---------------------------------------------------------------------------
// Varint helpers (protobuf-style LEB128 encoding)
// ---------------------------------------------------------------------------

/// Reads a varint64 from a byte stream. Returns `None` at EOF.
fn read_varint<R: Read>(reader: &mut R) -> PanResult<Option<u64>> {
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
fn write_varint(writer: &mut dyn Write, mut value: u64) -> PanResult<()> {
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

// ---------------------------------------------------------------------------
// VG framing protocol
// ---------------------------------------------------------------------------

/// Maximum number of nodes, edges or path mappings stored in one `Graph` message.
/// Matches vg's default chunk size and keeps every message far below protobuf's 2 GB limit.
const VG_CHUNK_SIZE: usize = 1000;

/// Maximum number of messages per framing group (libvgio uses the same limit).
const VG_GROUP_SIZE: usize = 1000;

/// Largest message length accepted when reading: protobuf messages are limited
/// to 2 GB, so a larger length means a corrupt stream, not a huge allocation.
const VG_MAX_MESSAGE_LEN: u64 = i32::MAX as u64;

/// Bytes of raw messages read before they are decoded together: hundreds of
/// vg's 1000-entity messages, so every thread gets work, while the two raw
/// windows in flight stay small next to the decoded graph.
const VG_READ_WINDOW: usize = 16 << 20;

/// Raw `Graph` messages, stored back to back.
#[derive(Default)]
struct RawWindow {
    bytes: Vec<u8>,
    messages: Vec<Range<usize>>,
}

impl RawWindow {
    #[cfg(test)]
    fn messages(&self) -> impl Iterator<Item = &[u8]> {
        self.messages.iter().map(|range| &self.bytes[range.clone()])
    }

    /// Decodes the messages in parallel batches, keeping stream order.
    fn decode(&self) -> PanResult<Vec<DecodedBatch>> {
        // A few batches per thread balance uneven messages.
        let batch_len = self
            .messages
            .len()
            .div_ceil(4 * rayon::current_num_threads())
            .max(1);
        self.messages
            .par_chunks(batch_len)
            .map(|ranges| {
                DecodedBatch::decode(ranges.iter().map(|range| &self.bytes[range.clone()]))
            })
            .collect()
    }
}

/// Splits a VG-framed protobuf stream into its `Graph` messages.
///
/// The VG framing format consists of groups. Each group starts with a varint
/// count of messages, each message is length-prefixed with a varint. The first
/// message in a group may be a type tag string (e.g. `"VG"`), which is skipped.
struct VgFrames<'r, R> {
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
            let msg_len = usize::try_from(msg_len)
                .ok()
                .filter(|_| msg_len <= VG_MAX_MESSAGE_LEN)
                .ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "VG message length {msg_len} exceeds the protobuf limit of {VG_MAX_MESSAGE_LEN} bytes"
                    ))
                })?;
            let start = window.bytes.len();
            window.bytes.resize(start + msg_len, 0);
            self.reader.read_exact(&mut window.bytes[start..])?;

            // Type tags are short ASCII strings; Graph messages are protobuf-encoded.
            if index == 0 {
                let (is_type_tag, tag_str) = detect_type_tag(&window.bytes[start..]);
                if is_type_tag {
                    debug!("VG stream type tag: {tag_str:?}");
                    window.bytes.truncate(start);
                    continue;
                }
            }
            window.messages.push(start..start + msg_len);
        }
        Ok(())
    }
}

/// Reads all `Graph` messages from a VG-framed protobuf stream.
#[cfg(test)]
fn read_vg_graphs<R: Read>(reader: &mut R) -> PanResult<Vec<vg_proto::Graph>> {
    let mut frames = VgFrames::new(reader);
    let mut graphs = Vec::new();
    let mut window = RawWindow::default();
    loop {
        frames.read_window(VG_READ_WINDOW, &mut window)?;
        if window.messages.is_empty() {
            return Ok(graphs);
        }
        for message in window.messages() {
            graphs.push(decode_graph(message)?);
        }
    }
}

/// Decodes one `Graph` message as `M` (the full `vg_proto::Graph` or the
/// reader's `vg_read::Graph`).
fn decode_graph<M: Message + Default>(message: &[u8]) -> PanResult<M> {
    M::decode(message)
        .map_err(|e| PanGraphXError::Parse(format!("Failed to decode VG Graph message: {e}")))
}

/// Heuristic to determine if a message buffer is a type tag string.
/// Type tags are short (typically <= 10 bytes), all printable ASCII.
fn detect_type_tag(buf: &[u8]) -> (bool, String) {
    if buf.len() <= 16 && !buf.is_empty() && buf.iter().all(|&b| b.is_ascii_graphic()) {
        let s = String::from_utf8_lossy(buf).to_string();
        return (true, s);
    }
    (false, String::new())
}

/// Writes one framing group: the `"VG"` type tag followed by already-encoded `Graph` messages.
fn write_vg_group(writer: &mut dyn Write, encoded_graphs: &[Vec<u8>]) -> PanResult<()> {
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
fn write_vg_graphs(writer: &mut dyn Write, graphs: &[vg_proto::Graph]) -> PanResult<()> {
    let encoded: Vec<Vec<u8>> = graphs.iter().map(Message::encode_to_vec).collect();
    write_vg_group(writer, &encoded)
}

// ---------------------------------------------------------------------------
// Orientation mapping between VG proto and internal representation
// ---------------------------------------------------------------------------

/// VG Edge orientation convention:
/// - `from_start = false` means leaving from the end of the node → Forward
/// - `from_start = true` means leaving from the start of the node → Reverse
/// - `to_end = false` means arriving at the start of the node → Forward
/// - `to_end = true` means arriving at the end of the node → Reverse
const fn vg_edge_to_orientations(from_start: bool, to_end: bool) -> (Orientation, Orientation) {
    let from_orient = if from_start {
        Orientation::Reverse
    } else {
        Orientation::Forward
    };
    let to_orient = if to_end {
        Orientation::Reverse
    } else {
        Orientation::Forward
    };
    (from_orient, to_orient)
}

fn orientations_to_vg_edge(from_orient: Orientation, to_orient: Orientation) -> (bool, bool) {
    let from_start = from_orient == Orientation::Reverse;
    let to_end = to_orient == Orientation::Reverse;
    (from_start, to_end)
}

// ---------------------------------------------------------------------------
// GraphParser implementation
// ---------------------------------------------------------------------------

/// A path step as read from VG: `node_id * 2 + is_reverse`, half the size of
/// the two fields. Unpacking is an arithmetic shift, so negative ids survive.
type PackedStep = i64;

/// Packs a VG node id and strand, or `None` if the id lies outside ±2^62.
/// handlegraph packs handles the same way, so real vg ids never get close.
fn pack_step(node_id: i64, is_reverse: bool) -> Option<PackedStep> {
    node_id
        .checked_mul(2)
        .map(|id| id | PackedStep::from(is_reverse))
}

/// The VG node id and strand of a packed step.
const fn unpack_step(step: PackedStep) -> (i64, bool) {
    (step >> 1, step & 1 == 1)
}

/// The mapping ranks of one path chunk.
enum ChunkRanks {
    /// `first, first + 1, …`, one per step; what vg and `PanGraphX` write.
    Consecutive(i64),
    /// Any other ranks, one per step.
    Explicit(Vec<i64>),
}

/// The part of one path stored in a single `Graph` message.
struct PathChunk {
    name: Vec<u8>,
    /// Number of steps; they are stored in the batch the chunk was decoded in.
    len: usize,
    ranks: ChunkRanks,
}

impl PathChunk {
    /// Decodes `path`, appending its steps to `steps`.
    fn decode(path: vg_read::Path, steps: &mut Vec<PackedStep>) -> PanResult<Self> {
        let name_error = |message: &str| {
            PanGraphXError::Parse(format!(
                "Path {}: {message}",
                String::from_utf8_lossy(&path.name)
            ))
        };
        let first = path.mapping.first().map_or(0, |m| m.rank);
        let mut next_rank = Some(first);
        let mut consecutive = true;
        steps.reserve(path.mapping.len());
        for mapping in &path.mapping {
            let position = mapping
                .position
                .as_ref()
                .ok_or_else(|| name_error("mapping is missing its position"))?;
            let step = pack_step(position.node_id, position.is_reverse).ok_or_else(|| {
                name_error(&format!(
                    "VG node id {} is outside the supported range ±2^62",
                    position.node_id
                ))
            })?;
            steps.push(step);
            consecutive &= next_rank == Some(mapping.rank);
            next_rank = next_rank.and_then(|rank| rank.checked_add(1));
        }
        let ranks = if consecutive {
            ChunkRanks::Consecutive(first)
        } else {
            ChunkRanks::Explicit(path.mapping.iter().map(|m| m.rank).collect())
        };
        Ok(Self {
            name: path.name,
            len: path.mapping.len(),
            ranks,
        })
    }
}

/// What the mapping ranks of a path read so far say about its step order.
enum PathRanks {
    /// Every rank equals its 1-based position, so the steps are in rank
    /// order and no rank needs storing.
    Implicit,
    /// A rank is unset (≤ 0) or repeated, so ranks define no order and
    /// stream order is kept.
    StreamOrder,
    /// Positive ranks that may reorder the path, one per step.
    Explicit(Vec<i64>),
}

/// Mappings of one path, accumulated across all `Graph` chunks it is split over.
struct VgPath {
    name: Vec<u8>,
    steps: Vec<PackedStep>,
    ranks: PathRanks,
}

impl VgPath {
    const fn new(name: Vec<u8>) -> Self {
        Self {
            name,
            steps: Vec::new(),
            ranks: PathRanks::Implicit,
        }
    }

    /// Appends the next chunk of this path in stream order.
    fn append(&mut self, ranks: ChunkRanks, steps: &[PackedStep]) {
        let start = self.steps.len();
        match ranks {
            ChunkRanks::Consecutive(first) => {
                let continues = matches!(self.ranks, PathRanks::Implicit)
                    && usize::try_from(first).is_ok_and(|first| first == start + 1);
                if !continues && !matches!(self.ranks, PathRanks::StreamOrder) {
                    let ranks = std::iter::successors(Some(first), |rank| rank.checked_add(1));
                    for (offset, rank) in ranks.take(steps.len()).enumerate() {
                        self.push_rank(start + offset, rank);
                    }
                }
            }
            ChunkRanks::Explicit(ranks) => {
                for (offset, rank) in ranks.into_iter().enumerate() {
                    self.push_rank(start + offset, rank);
                }
            }
        }
        self.steps.extend_from_slice(steps);
    }

    /// Records the rank of the step at 0-based `position`.
    fn push_rank(&mut self, position: usize, rank: i64) {
        let next = match &mut self.ranks {
            PathRanks::StreamOrder => return,
            _ if rank <= 0 => PathRanks::StreamOrder,
            PathRanks::Explicit(ranks) => {
                ranks.push(rank);
                return;
            }
            PathRanks::Implicit => match usize::try_from(rank) {
                Ok(rank) if rank == position + 1 => return,
                // Ranks `1..=position` already belong to the earlier steps.
                Ok(rank) if rank <= position => {
                    debug!(
                        "VG path {} has repeated mapping ranks, keeping stream order",
                        String::from_utf8_lossy(&self.name)
                    );
                    PathRanks::StreamOrder
                }
                _ => {
                    let mut ranks: Vec<i64> = (1..).take(position).collect();
                    ranks.push(rank);
                    PathRanks::Explicit(ranks)
                }
            },
        };
        self.ranks = next;
    }

    /// Converts the path with `convert`, in path order: sorted by rank when
    /// every mapping carries a distinct positive one, otherwise in stream order.
    fn into_path(self, convert: impl Fn(PackedStep) -> PanResult<Step>) -> PanResult<Path> {
        let Self {
            name,
            mut steps,
            ranks,
        } = self;
        if let PathRanks::Explicit(ranks) = ranks
            && !ranks.is_sorted()
        {
            let mut ranked: Vec<(i64, PackedStep)> =
                ranks.into_iter().zip(steps.iter().copied()).collect();
            ranked.sort_unstable_by_key(|&(rank, _)| rank);
            if ranked.windows(2).any(|w| w[0].0 == w[1].0) {
                debug!(
                    "VG path {} has repeated mapping ranks, keeping stream order",
                    String::from_utf8_lossy(&name)
                );
            } else {
                steps = ranked.into_iter().map(|(_, step)| step).collect();
            }
        }
        // Collecting from a borrowing iterator gives the steps an allocation of
        // their own; the packed steps are freed on return.
        let steps = steps
            .iter()
            .map(|&step| convert(step))
            .collect::<PanResult<Vec<Step>>>()?;
        Ok(Path {
            name,
            steps,
            overlaps: Vec::new(),
        })
    }
}

/// The contents of a run of consecutive `Graph` messages, in stream order.
///
/// Steps of all path chunks share one buffer, so a batch makes a few large
/// allocations instead of one per message.
#[derive(Default)]
struct DecodedBatch {
    nodes: Vec<vg_read::Node>,
    edges: Vec<vg_read::Edge>,
    paths: Vec<PathChunk>,
    /// The steps of `paths`, back to back.
    steps: Vec<PackedStep>,
}

impl DecodedBatch {
    fn decode<'a>(messages: impl Iterator<Item = &'a [u8]>) -> PanResult<Self> {
        let mut batch = Self::default();
        for message in messages {
            let graph: vg_read::Graph = decode_graph(message)?;
            batch.nodes.extend(graph.node);
            batch.edges.extend(graph.edge);
            for path in graph.path {
                batch.paths.push(PathChunk::decode(path, &mut batch.steps)?);
            }
        }
        Ok(batch)
    }
}

/// Nodes, edges and paths of a whole VG stream, merged across its `Graph` chunks.
#[derive(Default)]
struct VgStream {
    nodes: Vec<vg_read::Node>,
    edges: Vec<vg_read::Edge>,
    paths: Vec<VgPath>,
    /// Path name → index in `paths`.
    path_index: HashMap<Vec<u8>, usize>,
}

impl VgStream {
    /// Collects nodes, edges and path steps from all partial graphs.
    ///
    /// The stream is read in windows of about `window` bytes. While one window
    /// is decoded in parallel the next one is read, and decoded chunks are
    /// merged in stream order, so the result does not depend on the thread
    /// count. A stream of one huge message decodes on a single thread.
    fn read<R: Read>(reader: &mut R, window: usize) -> PanResult<Self> {
        let mut frames = VgFrames::new(reader);
        let mut stream = Self::default();
        let mut current = RawWindow::default();
        let mut next = RawWindow::default();
        frames.read_window(window, &mut current)?;
        while !current.messages.is_empty() {
            let mut decoded = Ok(Vec::new());
            // `in_place_scope` keeps the reader on this thread, so it need not be `Send`.
            let read = rayon::in_place_scope(|scope| {
                scope.spawn(|_| decoded = current.decode());
                frames.read_window(window, &mut next)
            });
            for batch in decoded? {
                stream.merge(batch);
            }
            read?;
            std::mem::swap(&mut current, &mut next);
        }
        Ok(stream)
    }

    /// Adds the next batch in stream order.
    fn merge(&mut self, batch: DecodedBatch) {
        self.nodes.extend(batch.nodes);
        self.edges.extend(batch.edges);
        let mut steps = batch.steps.as_slice();
        for mut path in batch.paths {
            let (path_steps, rest) = steps.split_at(path.len);
            steps = rest;
            let index = if let Some(&index) = self.path_index.get(&path.name) {
                index
            } else {
                self.path_index.insert(path.name.clone(), self.paths.len());
                self.paths.push(VgPath::new(std::mem::take(&mut path.name)));
                self.paths.len() - 1
            };
            self.paths[index].append(path.ranks, path_steps);
        }
    }
}

impl<R: Read + Seek> GraphParser<R> for VGCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        parse_vg(reader, VG_READ_WINDOW)
    }
}

/// Parses a VG stream, decoding about `window` bytes of messages at a time.
fn parse_vg<R: Read>(reader: &mut R, window: usize) -> PanResult<CoreGraphDTO> {
    let VgStream {
        nodes: mut proto_nodes,
        edges: proto_edges,
        paths: vg_paths,
        path_index,
    } = VgStream::read(reader, window)?;
    drop(path_index);

    // Build node ID mapping: VG i64 IDs → 0-based usize
    // Sort by original ID to maintain a deterministic ordering. Writers usually
    // emit nodes in id order already. Stability does not matter: nodes sharing
    // an id must be identical, which is checked next.
    if !proto_nodes.is_sorted_by_key(|n| n.id) {
        proto_nodes.par_sort_unstable_by_key(|n| n.id);
    }
    // A node repeated across chunks is merged; an id reused for a different
    // node is an error rather than silently dropping one of them.
    if let Some(pair) = proto_nodes
        .windows(2)
        .find(|pair| pair[0].id == pair[1].id && pair[0] != pair[1])
    {
        return Err(PanGraphXError::Parse(format!(
            "Duplicate VG node ID {} with conflicting sequence or name",
            pair[0].id
        )));
    }
    proto_nodes.dedup_by_key(|n| n.id);
    let vg_ids: Vec<i64> = proto_nodes.iter().map(|n| n.id).collect();
    // Ids were deduplicated above, so a duplicate cannot occur here.
    let id_map = IdMap::from_ids(&vg_ids)
        .map_err(|id| PanGraphXError::Parse(format!("Duplicate VG node ID {id}")))?;
    drop(vg_ids);

    let mut node_name_map: HashMap<usize, Vec<u8>> = HashMap::with_capacity(proto_nodes.len());
    let mut nodes = Nodes::new();
    for (idx, proto_node) in proto_nodes.into_iter().enumerate() {
        // Store original name or ID as the node name
        let name = if proto_node.name.is_empty() {
            itoa::Buffer::new()
                .format(proto_node.id)
                .as_bytes()
                .to_vec()
        } else {
            proto_node.name
        };
        node_name_map.insert(idx, name);
        nodes.push(proto_node.sequence);
    }

    // Convert edges
    #[allow(clippy::cast_sign_loss)]
    let edges: Vec<Edge> = proto_edges
        .par_iter()
        .map(|pe| {
            let from_id = id_map.get(pe.from).ok_or_else(|| {
                PanGraphXError::Parse(format!("Edge references unknown from-node ID {}", pe.from))
            })?;
            let to_id = id_map.get(pe.to).ok_or_else(|| {
                PanGraphXError::Parse(format!("Edge references unknown to-node ID {}", pe.to))
            })?;
            let (from_orient, to_orient) = vg_edge_to_orientations(pe.from_start, pe.to_end);
            Ok(Edge {
                from_node: from_id,
                from_orient,
                to_node: to_id,
                to_orient,
                overlap: pe.overlap as u32,
            })
        })
        .collect::<PanResult<Vec<Edge>>>()?;
    drop(proto_edges);

    // Convert paths
    let paths = vg_paths
        .into_par_iter()
        .map(|vg_path| {
            vg_path.into_path(|step| {
                let (vg_id, is_reverse) = unpack_step(step);
                let node_id = id_map.get(vg_id).ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "Path mapping references unknown node ID {vg_id}"
                    ))
                })?;
                Step::new(node_id, is_reverse.into())
            })
        })
        .collect::<PanResult<Vec<Path>>>()?;

    Ok(CoreGraphDTO {
        nodes,
        edges,
        paths,
        node_name_map: Some(node_name_map),
    })
}

// ---------------------------------------------------------------------------
// GraphSerializer implementation
// ---------------------------------------------------------------------------

/// The slice of the graph stored in one `Graph` message.
///
/// Nodes come first, then edges, then paths, so every message only refers to
/// nodes a reader has already seen.
enum VgChunk<'a> {
    Nodes(&'a [Node]),
    Edges(&'a [Edge]),
    /// Steps `start..end` of `path`.
    Path {
        path: &'a Path,
        start: usize,
        end: usize,
    },
}

/// Splits the graph into chunks of at most `VG_CHUNK_SIZE` entities.
fn vg_chunks(graph: &CoreGraphDTO) -> Vec<VgChunk<'_>> {
    let mut chunks: Vec<VgChunk<'_>> = graph
        .nodes
        .as_slice()
        .chunks(VG_CHUNK_SIZE)
        .map(VgChunk::Nodes)
        .collect();
    chunks.extend(graph.edges.chunks(VG_CHUNK_SIZE).map(VgChunk::Edges));
    for path in &graph.paths {
        // An empty path still gets one (empty) chunk so that its name is kept.
        let len = path.steps.len();
        let mut start = 0;
        loop {
            let end = (start + VG_CHUNK_SIZE).min(len);
            chunks.push(VgChunk::Path { path, start, end });
            start = end;
            if start >= len {
                break;
            }
        }
    }
    chunks
}

/// Lookup tables shared by all chunks while serializing.
struct VgWriteContext<'a> {
    graph: &'a CoreGraphDTO,
    /// Internal id → VG id.
    vg_ids: Vec<i64>,
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
impl<'a> VgWriteContext<'a> {
    /// Keeps the node names as VG ids when every name is a distinct, positive,
    /// canonically written integer; otherwise all nodes get ids `1..=n`.
    ///
    /// Choosing per node would let a kept id collide with an assigned one (`2`
    /// and `x` at index 1 both becoming id 2). Renumbered nodes keep their
    /// names in the VG `name` field.
    fn new(graph: &'a CoreGraphDTO) -> Self {
        let n = graph.nodes.len();
        let named = graph.node_name_map.as_ref().and_then(|map| {
            let ids: Option<Vec<i64>> = (0..n)
                .into_par_iter()
                .map(|id| parse_canonical_int(map.get(&id)?).filter(|&vg_id| vg_id > 0))
                .collect();
            ids.filter(|ids| IdMap::from_ids(ids).is_ok())
        });
        let vg_ids = named.unwrap_or_else(|| {
            debug!("Node names are not distinct positive integers; VG ids are 1..={n}");
            (1..=n as i64).collect()
        });
        Self { graph, vg_ids }
    }

    fn node(&self, node: &Node) -> vg_proto::Node {
        let vg_id = self.vg_ids[node.id];
        // Only set name if it differs from the ID string
        let name = self
            .graph
            .node_name_map
            .as_ref()
            .and_then(|m| m.get(&node.id))
            .filter(|name| name.as_slice() != itoa::Buffer::new().format(vg_id).as_bytes())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .unwrap_or_default();
        vg_proto::Node {
            sequence: String::from_utf8_lossy(&node.sequence).into_owned(),
            name,
            id: vg_id,
        }
    }

    fn edge(&self, edge: &Edge) -> vg_proto::Edge {
        let (from_start, to_end) = orientations_to_vg_edge(edge.from_orient, edge.to_orient);
        vg_proto::Edge {
            from: self.vg_ids[edge.from_node],
            to: self.vg_ids[edge.to_node],
            from_start,
            to_end,
            overlap: edge.overlap as i32,
        }
    }

    fn path(&self, path: &Path, start: usize, end: usize) -> vg_proto::Path {
        let mapping = path.steps[start..end]
            .iter()
            .zip(start..)
            .map(|(step, index)| {
                let node_seq_len = self
                    .graph
                    .nodes
                    .get(step.node_id())
                    .map_or(0, |n| n.sequence.len()) as i32;
                vg_proto::Mapping {
                    position: Some(vg_proto::Position {
                        node_id: self.vg_ids[step.node_id()],
                        offset: 0,
                        is_reverse: step.is_reverse(),
                        name: String::new(),
                    }),
                    edit: vec![vg_proto::Edit {
                        from_length: node_seq_len,
                        to_length: node_seq_len,
                        sequence: String::new(),
                    }],
                    rank: (index + 1) as i64,
                }
            })
            .collect();
        vg_proto::Path {
            name: String::from_utf8_lossy(&path.name).into_owned(),
            mapping,
            is_circular: false,
            length: 0,
        }
    }

    fn encode(&self, chunk: &VgChunk<'_>) -> Vec<u8> {
        let graph = match *chunk {
            VgChunk::Nodes(nodes) => vg_proto::Graph {
                node: nodes.iter().map(|n| self.node(n)).collect(),
                ..Default::default()
            },
            VgChunk::Edges(edges) => vg_proto::Graph {
                edge: edges.iter().map(|e| self.edge(e)).collect(),
                ..Default::default()
            },
            VgChunk::Path { path, start, end } => vg_proto::Graph {
                path: vec![self.path(path, start, end)],
                ..Default::default()
            },
        };
        graph.encode_to_vec()
    }
}

impl GraphSerializer for VGCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        if let Some(message) = graph.dangling_reference() {
            return Err(PanGraphXError::Serialize(message));
        }
        let context = VgWriteContext::new(graph);
        let chunks = vg_chunks(graph);

        // Encode one group of chunks in parallel at a time and write it in order,
        // so only a bounded window of protobuf data is ever held in memory.
        if chunks.is_empty() {
            // Keep the stream non-empty so that readers see a valid VG file.
            return write_vg_group(writer, &[vg_proto::Graph::default().encode_to_vec()]);
        }
        for group in chunks.chunks(VG_GROUP_SIZE) {
            let encoded: Vec<Vec<u8>> = group.par_iter().map(|c| context.encode(c)).collect();
            write_vg_group(writer, &encoded)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
    use crate::core::graph_dto::CoreGraphDTO;
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

    // ---- Orientation mapping tests ----

    #[test]
    fn test_edge_orientation_mapping() {
        // Default: from_start=false, to_end=false → Forward, Forward
        assert_eq!(
            vg_edge_to_orientations(false, false),
            (Orientation::Forward, Orientation::Forward)
        );
        // from_start=true, to_end=false → Reverse, Forward
        assert_eq!(
            vg_edge_to_orientations(true, false),
            (Orientation::Reverse, Orientation::Forward)
        );
        // from_start=false, to_end=true → Forward, Reverse
        assert_eq!(
            vg_edge_to_orientations(false, true),
            (Orientation::Forward, Orientation::Reverse)
        );
        // from_start=true, to_end=true → Reverse, Reverse
        assert_eq!(
            vg_edge_to_orientations(true, true),
            (Orientation::Reverse, Orientation::Reverse)
        );
    }

    #[test]
    fn test_orientation_roundtrip() {
        let cases = [
            (Orientation::Forward, Orientation::Forward),
            (Orientation::Forward, Orientation::Reverse),
            (Orientation::Reverse, Orientation::Forward),
            (Orientation::Reverse, Orientation::Reverse),
        ];
        for (from_o, to_o) in cases {
            let (fs, te) = orientations_to_vg_edge(from_o, to_o);
            let (from_o2, to_o2) = vg_edge_to_orientations(fs, te);
            assert_eq!(from_o, from_o2);
            assert_eq!(to_o, to_o2);
        }
    }

    // ---- Parse / Serialize round-trip tests ----

    fn make_simple_graph() -> CoreGraphDTO {
        let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"TTGG".to_vec(), b"CCAA".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());
        name_map.insert(2, b"3".to_vec());

        CoreGraphDTO {
            nodes,
            edges: vec![
                Edge {
                    from_node: 0,
                    from_orient: Orientation::Forward,
                    to_node: 1,
                    to_orient: Orientation::Forward,
                    overlap: 0,
                },
                Edge {
                    from_node: 1,
                    from_orient: Orientation::Forward,
                    to_node: 2,
                    to_orient: Orientation::Forward,
                    overlap: 2,
                },
            ],
            paths: vec![Path {
                name: b"path1".to_vec(),
                steps: vec![
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(1, Orientation::Forward).unwrap(),
                    Step::new(2, Orientation::Reverse).unwrap(),
                ],
                overlaps: vec![],
            }],
            node_name_map: Some(name_map),
        }
    }

    #[test]
    fn test_serialize_then_parse_roundtrip() {
        let original = make_simple_graph();
        let codec = VGCodec;

        // Serialize
        let mut buf = Vec::new();
        codec.serialize(&original, &mut buf).unwrap();
        assert!(!buf.is_empty(), "Serialized VG should not be empty");

        // Parse back
        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        // Verify nodes
        assert_eq!(parsed.nodes.len(), original.nodes.len());
        for (i, node) in parsed.nodes.iter().enumerate() {
            assert_eq!(
                node.sequence, original.nodes[i].sequence,
                "Node {i} sequence mismatch"
            );
        }

        // Verify edges
        assert_eq!(parsed.edges.len(), original.edges.len());
        for (i, edge) in parsed.edges.iter().enumerate() {
            assert_eq!(edge.from_node, original.edges[i].from_node);
            assert_eq!(edge.to_node, original.edges[i].to_node);
            assert_eq!(edge.from_orient, original.edges[i].from_orient);
            assert_eq!(edge.to_orient, original.edges[i].to_orient);
            assert_eq!(edge.overlap, original.edges[i].overlap);
        }

        // Verify paths
        assert_eq!(parsed.paths.len(), original.paths.len());
        assert_eq!(parsed.paths[0].name, original.paths[0].name);
        assert_eq!(parsed.paths[0].steps.len(), original.paths[0].steps.len());
        for (i, step) in parsed.paths[0].steps.iter().enumerate() {
            assert_eq!(step.node_id(), original.paths[0].steps[i].node_id());
            assert_eq!(step.orientation(), original.paths[0].steps[i].orientation());
        }
    }

    #[test]
    fn test_empty_graph_roundtrip() {
        let graph = CoreGraphDTO::default();
        let codec = VGCodec;

        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.nodes.len(), 0);
        assert_eq!(parsed.edges.len(), 0);
        assert_eq!(parsed.paths.len(), 0);
    }

    #[test]
    fn test_single_node_no_edges() {
        let nodes = Nodes::from_seq(vec![b"GATTACA".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"42".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].sequence, b"GATTACA");
        assert_eq!(parsed.edges.len(), 0);
    }

    /// A chain of single-base nodes named `names`, with a path through all of them.
    fn named_chain(names: &[&str]) -> CoreGraphDTO {
        let bases = [b"A", b"C", b"G", b"T"];
        CoreGraphDTO {
            nodes: Nodes::from_seq((0..names.len()).map(|i| bases[i % 4].to_vec()).collect()),
            edges: (1..names.len())
                .map(|i| Edge {
                    from_node: i - 1,
                    from_orient: Orientation::Forward,
                    to_node: i,
                    to_orient: Orientation::Forward,
                    overlap: 0,
                })
                .collect(),
            paths: vec![Path {
                name: b"p".to_vec(),
                steps: (0..names.len())
                    .map(|i| Step::new(i, Orientation::Forward).unwrap())
                    .collect(),
                overlaps: vec![],
            }],
            node_name_map: Some(
                names
                    .iter()
                    .enumerate()
                    .map(|(i, name)| (i, name.as_bytes().to_vec()))
                    .collect(),
            ),
        }
    }

    /// The VG ids `graph` is written with; also checks the graph round-trips.
    fn written_ids(graph: &CoreGraphDTO) -> Vec<i64> {
        let mut buf = Vec::new();
        VGCodec.serialize(graph, &mut buf).unwrap();
        let parsed = VGCodec.parse(&mut Cursor::new(&buf)).unwrap();
        assert!(parsed.isomorphic(graph), "VG round-trip changed the graph");
        VgWriteContext::new(graph).vg_ids
    }

    #[test]
    fn test_distinct_positive_names_keep_ids() {
        assert_eq!(written_ids(&named_chain(&["7", "3", "1"])), vec![7, 3, 1]);
    }

    #[test]
    fn test_mixed_names_are_renumbered() {
        // Per-node ids would give both `2` and `x` (index 1) the VG id 2.
        assert_eq!(written_ids(&named_chain(&["2", "x"])), vec![1, 2]);
    }

    /// Parses a stream of one-node chunks.
    fn parse_chunks(nodes: &[(i64, &str)]) -> PanResult<CoreGraphDTO> {
        let graphs: Vec<vg_proto::Graph> = nodes
            .iter()
            .map(|&(id, sequence)| vg_proto::Graph {
                node: vec![vg_proto::Node {
                    id,
                    sequence: sequence.to_string(),
                    name: String::new(),
                }],
                ..Default::default()
            })
            .collect();
        let mut buf = Vec::new();
        write_vg_graphs(&mut buf, &graphs).unwrap();
        VGCodec.parse(&mut Cursor::new(&buf))
    }

    #[test]
    fn test_identical_repeated_node_is_merged() {
        let parsed = parse_chunks(&[(1, "ACGT"), (2, "GG"), (1, "ACGT")]).unwrap();
        assert_eq!(parsed.nodes.len(), 2);
    }

    #[test]
    fn test_conflicting_duplicate_id_is_rejected() {
        let error = parse_chunks(&[(1, "ACGT"), (1, "CCCC")]).unwrap_err();
        assert!(
            error.to_string().contains("Duplicate VG node ID 1"),
            "{error}"
        );
    }

    #[test]
    fn test_invalid_or_repeated_ids_force_renumbering() {
        for names in [&["0", "5"][..], &["-3", "5"], &["007", "5"], &["7", "007"]] {
            assert_eq!(written_ids(&named_chain(names)), vec![1, 2], "{names:?}");
        }
    }

    #[test]
    fn test_all_orientation_combinations() {
        let nodes = Nodes::from_seq(vec![b"AA".to_vec(), b"CC".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());

        let orientations = [
            (Orientation::Forward, Orientation::Forward),
            (Orientation::Forward, Orientation::Reverse),
            (Orientation::Reverse, Orientation::Forward),
            (Orientation::Reverse, Orientation::Reverse),
        ];

        for (from_o, to_o) in orientations {
            let graph = CoreGraphDTO {
                nodes: nodes.clone(),
                edges: vec![Edge {
                    from_node: 0,
                    from_orient: from_o,
                    to_node: 1,
                    to_orient: to_o,
                    overlap: 0,
                }],
                paths: vec![],
                node_name_map: Some(name_map.clone()),
            };

            let codec = VGCodec;
            let mut buf = Vec::new();
            codec.serialize(&graph, &mut buf).unwrap();

            let mut cursor = Cursor::new(&buf);
            let parsed = codec.parse(&mut cursor).unwrap();

            assert_eq!(parsed.edges.len(), 1);
            assert_eq!(
                parsed.edges[0].from_orient, from_o,
                "from_orient mismatch for {from_o:?} → {to_o:?}"
            );
            assert_eq!(
                parsed.edges[0].to_orient, to_o,
                "to_orient mismatch for {from_o:?} → {to_o:?}"
            );
        }
    }

    #[test]
    fn test_path_with_reverse_steps() {
        let nodes = Nodes::from_seq(vec![b"AAA".to_vec(), b"CCC".to_vec(), b"GGG".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());
        name_map.insert(2, b"3".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![Path {
                name: b"haplotype_1".to_vec(),
                steps: vec![
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(1, Orientation::Reverse).unwrap(),
                    Step::new(2, Orientation::Forward).unwrap(),
                ],
                overlaps: vec![],
            }],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.paths.len(), 1);
        assert_eq!(parsed.paths[0].steps[0].orientation(), Orientation::Forward);
        assert_eq!(parsed.paths[0].steps[1].orientation(), Orientation::Reverse);
        assert_eq!(parsed.paths[0].steps[2].orientation(), Orientation::Forward);
    }

    #[test]
    fn test_graph_without_name_map() {
        let nodes = Nodes::from_seq(vec![b"AT".to_vec(), b"GC".to_vec()]);
        let graph = CoreGraphDTO {
            nodes,
            edges: vec![Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 0,
            }],
            paths: vec![],
            node_name_map: None,
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.nodes.len(), 2);
        assert_eq!(parsed.nodes[0].sequence, b"AT");
        assert_eq!(parsed.nodes[1].sequence, b"GC");
        assert_eq!(parsed.edges.len(), 1);
    }

    #[test]
    fn test_multiple_paths() {
        let nodes = Nodes::from_seq(vec![b"A".to_vec(), b"C".to_vec(), b"G".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());
        name_map.insert(2, b"3".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![],
            paths: vec![
                Path {
                    name: b"ref".to_vec(),
                    steps: vec![
                        Step::new(0, Orientation::Forward).unwrap(),
                        Step::new(1, Orientation::Forward).unwrap(),
                    ],
                    overlaps: vec![],
                },
                Path {
                    name: b"alt".to_vec(),
                    steps: vec![
                        Step::new(0, Orientation::Forward).unwrap(),
                        Step::new(2, Orientation::Forward).unwrap(),
                    ],
                    overlaps: vec![],
                },
            ],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.paths.len(), 2);
        assert_eq!(parsed.paths[0].name, b"ref");
        assert_eq!(parsed.paths[1].name, b"alt");
        assert_eq!(parsed.paths[0].steps.len(), 2);
        assert_eq!(parsed.paths[1].steps.len(), 2);
    }

    #[test]
    fn test_edge_overlap_preserved() {
        let nodes = Nodes::from_seq(vec![b"ACGT".to_vec(), b"GTAA".to_vec()]);
        let mut name_map = HashMap::new();
        name_map.insert(0, b"1".to_vec());
        name_map.insert(1, b"2".to_vec());

        let graph = CoreGraphDTO {
            nodes,
            edges: vec![Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Forward,
                overlap: 2,
            }],
            paths: vec![],
            node_name_map: Some(name_map),
        };

        let codec = VGCodec;
        let mut buf = Vec::new();
        codec.serialize(&graph, &mut buf).unwrap();

        let mut cursor = Cursor::new(&buf);
        let parsed = codec.parse(&mut cursor).unwrap();

        assert_eq!(parsed.edges[0].overlap, 2);
    }

    #[test]
    fn test_large_graph_is_chunked_and_roundtrips() {
        // Enough nodes, edges and steps to span several messages and framing groups.
        // Nodes, edges and steps each take a third of the messages.
        let node_count = VG_CHUNK_SIZE * VG_GROUP_SIZE / 3 + 1500;
        let nodes = Nodes::from_seq(vec![b"A".to_vec(); node_count]);
        let edges: Vec<Edge> = (1..node_count)
            .map(|i| Edge {
                from_node: i - 1,
                from_orient: Orientation::Forward,
                to_node: i,
                to_orient: Orientation::Forward,
                overlap: 0,
            })
            .collect();
        let steps: Vec<Step> = (0..node_count)
            .map(|i| Step::new(i, (i % 3 == 0).into()).unwrap())
            .collect();
        let original = CoreGraphDTO {
            nodes,
            edges,
            paths: vec![
                Path {
                    name: b"long".to_vec(),
                    steps,
                    overlaps: vec![],
                },
                Path {
                    name: b"empty".to_vec(),
                    steps: vec![],
                    overlaps: vec![],
                },
            ],
            node_name_map: None,
        };

        let mut buf = Vec::new();
        VGCodec.serialize(&original, &mut buf).unwrap();

        let graphs = read_vg_graphs(&mut Cursor::new(&buf)).unwrap();
        assert!(graphs.len() > VG_GROUP_SIZE);
        assert!(graphs.iter().all(|g| g.node.len() <= VG_CHUNK_SIZE
            && g.edge.len() <= VG_CHUNK_SIZE
            && g.path.iter().all(|p| p.mapping.len() <= VG_CHUNK_SIZE)));

        let parsed = VGCodec.parse(&mut Cursor::new(&buf)).unwrap();
        assert!(parsed.isomorphic(&CoreGraphDTO {
            node_name_map: parsed.node_name_map.clone(),
            ..original
        }));
    }

    #[test]
    fn test_parallel_parse_is_deterministic() {
        let node_count = 5 * VG_CHUNK_SIZE + 7;
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq((0..node_count).map(|i| vec![b"ACGT"[i % 4]]).collect()),
            edges: (1..node_count)
                .map(|i| Edge {
                    from_node: i - 1,
                    from_orient: Orientation::Forward,
                    to_node: i,
                    to_orient: Orientation::Forward,
                    overlap: 0,
                })
                .collect(),
            paths: (0..3)
                .map(|p| Path {
                    name: format!("p{p}").into_bytes(),
                    steps: (p..node_count)
                        .step_by(p + 1)
                        .map(|i| Step::new(i, (i % 3 == 0).into()).unwrap())
                        .collect(),
                    overlaps: vec![],
                })
                .collect(),
            node_name_map: None,
        };
        let mut buf = Vec::new();
        VGCodec.serialize(&original, &mut buf).unwrap();

        let parse = |threads: usize, window: usize| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| parse_vg(&mut Cursor::new(&buf), window).unwrap())
        };
        let expected = parse(1, VG_READ_WINDOW);
        assert!(expected.isomorphic(&CoreGraphDTO {
            node_name_map: expected.node_name_map.clone(),
            ..original
        }));
        // Windows of one message each, and of a few messages, on several threads.
        for (threads, window) in [(1, 1), (4, 1), (4, 50_000), (3, VG_READ_WINDOW)] {
            assert_eq!(
                parse(threads, window),
                expected,
                "{threads} threads, window {window}"
            );
        }
    }

    /// A path read from chunks with the given mapping ranks; its steps visit
    /// nodes `1, 2, …` in stream order.
    fn ranked_path(chunks: &[&[i64]]) -> VgPath {
        let mut path = VgPath::new(b"p".to_vec());
        let mut node_id = 0;
        for ranks in chunks {
            let mapping = ranks
                .iter()
                .map(|&rank| {
                    node_id += 1;
                    vg_read::Mapping {
                        position: Some(vg_read::Position {
                            node_id,
                            is_reverse: false,
                        }),
                        rank,
                    }
                })
                .collect();
            let chunk = vg_read::Path {
                name: b"p".to_vec(),
                mapping,
            };
            let mut steps = Vec::new();
            let chunk = PathChunk::decode(chunk, &mut steps).unwrap();
            path.append(chunk.ranks, &steps);
        }
        path
    }

    fn node_order(path: VgPath) -> Vec<usize> {
        let path = path
            .into_path(|step| {
                let (vg_id, is_reverse) = unpack_step(step);
                Step::new(usize::try_from(vg_id).unwrap(), is_reverse.into())
            })
            .unwrap();
        path.steps.iter().map(|step| step.node_id()).collect()
    }

    #[test]
    fn test_sort_by_rank() {
        // Distinct ranks define the order, also across chunks.
        assert_eq!(node_order(ranked_path(&[&[3, 1, 2]])), vec![2, 3, 1]);
        assert_eq!(
            node_order(ranked_path(&[&[3, 4], &[1, 2]])),
            vec![3, 4, 1, 2]
        );

        // Unset ranks keep stream order.
        assert_eq!(node_order(ranked_path(&[&[2, 0, 1]])), vec![1, 2, 3]);

        // Increasing ranks with gaps are already in order.
        assert_eq!(node_order(ranked_path(&[&[2, 4], &[6]])), vec![1, 2, 3]);
        // Ranks repeated within one chunk keep stream order.
        assert_eq!(node_order(ranked_path(&[&[3, 3, 1]])), vec![1, 2, 3]);
    }

    #[test]
    fn test_positional_ranks_are_not_stored() {
        let path = ranked_path(&[&[1, 2], &[3, 4], &[], &[5]]);
        assert!(matches!(path.ranks, PathRanks::Implicit));
        assert_eq!(node_order(path), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_restarting_ranks_keep_stream_order() {
        // Ranks restarting in every chunk must not interleave the chunks, and
        // are recognized as repeated without storing them.
        let path = ranked_path(&[&[1, 2], &[1, 2]]);
        assert!(matches!(path.ranks, PathRanks::StreamOrder));
        assert_eq!(node_order(path), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_packed_steps() {
        for (node_id, is_reverse) in [(1, false), (7, true), (-3, true), (-(1 << 62), false)] {
            let step = pack_step(node_id, is_reverse).unwrap();
            assert_eq!(unpack_step(step), (node_id, is_reverse));
        }
        assert_eq!(pack_step(1 << 62, false), None);
    }

    #[test]
    fn test_out_of_range_path_node_id_is_rejected() {
        let graph = vg_proto::Graph {
            path: vec![vg_proto::Path {
                name: "p".to_string(),
                mapping: vec![vg_proto::Mapping {
                    position: Some(vg_proto::Position {
                        node_id: i64::MAX,
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut buf = Vec::new();
        write_vg_graphs(&mut buf, &[graph]).unwrap();
        let error = VGCodec.parse(&mut Cursor::new(&buf)).unwrap_err();
        assert!(
            error.to_string().contains("outside the supported range"),
            "{error}"
        );
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
