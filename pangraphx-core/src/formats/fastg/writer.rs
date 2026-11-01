//! FASTG writer: a forward and a reverse (`'`) record per node, `SPAdes` style.

use crate::core::core_types::{Edge, NodeId, Nodes, Orientation};
use crate::core::graph_dto::CoreGraphDTO;
use crate::core::graph_utils::oriented_bases;
use crate::error::PanResult;
use crate::formats::common::chunked::{RECORDS_PER_CHUNK, write_records};
use crate::formats::common::names::{node_names, one_based_names};
use log::warn;
use rayon::prelude::*;
use std::borrow::Cow;
use std::io::Write;

/// Whether `name` can be written inside a SPAdes-style record name: non-empty
/// printable ASCII without `_` (Bandage splits on it) or FASTG delimiters.
pub(super) fn is_record_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().all(|&b| {
            b.is_ascii_graphic() && !matches!(b, b'_' | b':' | b',' | b';' | b'\'' | b'>')
        })
}

/// Handle of a node strand: `node << 1 | is_reverse`.
#[inline]
pub(super) fn handle(node: NodeId, orientation: Orientation) -> usize {
    node << 1 | usize::from(orientation == Orientation::Reverse)
}

/// Successor lists of every node strand in CSR form. Each edge `a -> b` is
/// stored under `a` and, mirrored as `b' -> a'`, under `b'`. Rows are sorted
/// and may contain duplicates, which the writer skips.
pub(super) struct Successors {
    offsets: Vec<usize>,
    targets: Vec<usize>,
}

impl Successors {
    fn new(node_count: usize, edges: &[Edge]) -> Self {
        let pairs = || {
            edges.iter().flat_map(|edge| {
                let from = handle(edge.from_node, edge.from_orient);
                let to = handle(edge.to_node, edge.to_orient);
                [(from, to), (to ^ 1, from ^ 1)]
            })
        };

        let mut offsets = vec![0; 2 * node_count + 1];
        for (from, _) in pairs() {
            offsets[from + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }

        let mut cursor = offsets.clone();
        let mut targets = vec![0; 2 * edges.len()];
        for (from, to) in pairs() {
            targets[cursor[from]] = to;
            cursor[from] += 1;
        }
        drop(cursor);
        for row in offsets.windows(2) {
            targets[row[0]..row[1]].sort_unstable();
        }
        Self { offsets, targets }
    }

    fn of(&self, handle: usize) -> &[usize] {
        &self.targets[self.offsets[handle]..self.offsets[handle + 1]]
    }
}

/// Node names usable inside record names, falling back to `1..=n` for all
/// nodes if any name is not (so the fallback cannot collide with a kept name).
pub(super) fn record_names(graph: &CoreGraphDTO) -> Vec<Cow<'_, [u8]>> {
    let names = node_names(graph);
    if names.par_iter().all(|name| is_record_name(name)) {
        return names;
    }
    warn!(
        "Some node names contain characters FASTG reserves (`_`, `:`, `,`, `;`, `'`, `>` or whitespace); nodes are renamed to 1..={}.",
        graph.nodes().len()
    );
    one_based_names(graph)
}

pub(super) fn push_record_name(
    nodes: &Nodes,
    names: &[Cow<'_, [u8]>],
    strand: usize,
    buf: &mut Vec<u8>,
) {
    let id = strand >> 1;
    buf.extend_from_slice(b"EDGE_");
    buf.extend_from_slice(&names[id]);
    buf.extend_from_slice(b"_length_");
    buf.extend_from_slice(
        itoa::Buffer::new()
            .format(nodes[id].sequence.len())
            .as_bytes(),
    );
    buf.extend_from_slice(b"_cov_1");
    if strand & 1 == 1 {
        buf.push(b'\'');
    }
}

pub(super) fn format_record(
    nodes: &Nodes,
    names: &[Cow<'_, [u8]>],
    successors: &Successors,
    id: NodeId,
    orientation: Orientation,
    buf: &mut Vec<u8>,
) {
    let strand = handle(id, orientation);
    buf.push(b'>');
    push_record_name(nodes, names, strand, buf);
    let mut previous = None;
    for &target in successors.of(strand) {
        if previous == Some(target) {
            continue;
        }
        buf.push(if previous.is_none() { b':' } else { b',' });
        push_record_name(nodes, names, target, buf);
        previous = Some(target);
    }
    buf.extend_from_slice(b";\n");
    let node = &nodes[id];
    match orientation {
        Orientation::Forward => buf.extend_from_slice(&node.sequence),
        Orientation::Reverse => {
            buf.reserve(node.sequence.len());
            buf.extend(oriented_bases(&node.sequence, Orientation::Reverse, 0));
        }
    }
    buf.push(b'\n');
}

/// Target amount of text formatted per parallel task. Long nodes are written
/// twice (both strands), so chunks are sized by bytes rather than by count to
/// keep the buffered output window small.
pub(super) const CHUNK_BYTES: usize = 1 << 20;

/// Nodes per parallel task so that a chunk holds about [`CHUNK_BYTES`].
pub(super) fn nodes_per_chunk(nodes: &Nodes) -> usize {
    let sequence_bytes: usize = nodes
        .as_slice()
        .par_iter()
        .map(|node| node.sequence.len())
        .sum();
    let average = sequence_bytes / nodes.len().max(1);
    (CHUNK_BYTES / (2 * average + 1)).clamp(1, RECORDS_PER_CHUNK)
}

/// Writes `graph` as FASTG; paths and overlaps are dropped.
pub(super) fn write(graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
    if !graph.paths().is_empty() {
        warn!(
            "FASTG files don't support paths; {} paths are dropped.",
            graph.paths().len()
        );
    }
    if graph.edges().par_iter().any(|edge| edge.overlap != 0) {
        warn!("FASTG files don't support overlaps; edge overlaps are dropped.");
    }

    let names = record_names(graph);
    let successors = Successors::new(graph.nodes().len(), graph.edges());
    write_records(
        writer,
        graph.nodes().as_slice(),
        nodes_per_chunk(graph.nodes()),
        |id, _, buf| {
            for orientation in [Orientation::Forward, Orientation::Reverse] {
                format_record(graph.nodes(), &names, &successors, id, orientation, buf);
            }
            Ok(())
        },
    )
}
