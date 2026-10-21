//! Reader for vg's `HashGraph` format (`vg convert -a`, usually `.hg` or `.vg`).
//!
//! The layout mirrors libbdsg's `HashGraph::serialize_members`. Unlike
//! `PackedGraph`, every integer is **big-endian**, and nodes and paths come in
//! hash table order:
//!
//! ```text
//! u32      magic (676155192)
//! i64 ×3   max_id, min_id, next_path_id
//! u64      node count, then per node:
//!   i64 id, u64 length, sequence bytes,
//!   u64 n, n × i64 left edge handle, u64 m, m × i64 right edge handle
//! u64      path count, then per path:
//!   u8 is_circular, i64 path id, u64 name length, name bytes,
//!   u64 step count, steps × i64 handle
//! ```
//!
//! A handle is `id << 1 | is_reverse`. The right edges of a node are the
//! handles reached leaving it forward, the left edges those reached leaving
//! it in reverse; as in `PackedGraph`, every edge is listed on both of its
//! sides except a reversing self-edge. Sequences are kept byte for byte.

use crate::core::core_types::{Edge, NodeId, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::binio::{self, MAX_PREALLOC};
use crate::formats::id_map::IdMap;
use crate::formats::packed_graph_format::is_canonical_copy;
use log::warn;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Read;

/// libhandlegraph magic number of `bdsg::HashGraph`.
pub const HASH_GRAPH_MAGIC: u32 = 676_155_192;

/// Format name used in error messages.
const FORMAT: &str = "HashGraph";

fn read_u64<R: Read>(reader: &mut R) -> PanResult<u64> {
    Ok(u64::from_be_bytes(binio::read_array(reader, FORMAT)?))
}

fn read_count<R: Read>(reader: &mut R, what: &str) -> PanResult<usize> {
    binio::to_usize(read_u64(reader)?, FORMAT, what)
}

fn read_handles<R: Read>(reader: &mut R) -> PanResult<Vec<u64>> {
    let count = read_count(reader, "handle count")?;
    let mut handles = Vec::with_capacity(count.min(MAX_PREALLOC));
    for _ in 0..count {
        handles.push(read_u64(reader)?);
    }
    Ok(handles)
}

struct HashNode {
    id: i64,
    sequence: Vec<u8>,
    left: Vec<u64>,
    right: Vec<u64>,
}

struct HashPath {
    id: i64,
    name: Vec<u8>,
    circular: bool,
    steps: Vec<u64>,
}

fn read_nodes<R: Read>(reader: &mut R) -> PanResult<Vec<HashNode>> {
    let node_count = read_count(reader, "node count")?;
    let mut nodes = Vec::with_capacity(node_count.min(MAX_PREALLOC));
    for _ in 0..node_count {
        let id = read_u64(reader)?.cast_signed();
        let len = read_u64(reader)?;
        let sequence = binio::read_bytes(reader, len, FORMAT)?;
        let left = read_handles(reader)?;
        let right = read_handles(reader)?;
        nodes.push(HashNode {
            id,
            sequence,
            left,
            right,
        });
    }
    Ok(nodes)
}

fn read_paths<R: Read>(reader: &mut R) -> PanResult<Vec<HashPath>> {
    let path_count = read_count(reader, "path count")?;
    let mut paths = Vec::with_capacity(path_count.min(MAX_PREALLOC));
    for _ in 0..path_count {
        let circular = binio::read_array::<1, _>(reader, FORMAT)?[0] != 0;
        let id = read_u64(reader)?.cast_signed();
        let name_len = read_u64(reader)?;
        let name = binio::read_bytes(reader, name_len, FORMAT)?;
        let steps = read_handles(reader)?;
        paths.push(HashPath {
            id,
            name,
            circular,
            steps,
        });
    }
    Ok(paths)
}

/// Reads a bare `HashGraph` file, starting at its magic number.
///
/// # Errors
/// Returns a parse error for a truncated or malformed file, or an I/O error.
pub fn read_hash_graph<R: Read>(reader: &mut R) -> PanResult<CoreGraphDTO> {
    let magic = u32::from_be_bytes(binio::read_array(reader, FORMAT)?);
    if magic != HASH_GRAPH_MAGIC {
        return Err(PanGraphXError::Parse(format!(
            "Not a {FORMAT} file (magic number {magic})"
        )));
    }
    let _max_id = read_u64(reader)?;
    let _min_id = read_u64(reader)?;
    let _next_path_id = read_u64(reader)?;

    let mut nodes = read_nodes(reader)?;
    let mut paths = read_paths(reader)?;

    // Hash table order is arbitrary; sort for deterministic output.
    nodes.sort_unstable_by_key(|node| node.id);
    paths.sort_unstable_by_key(|path| path.id);
    let ids: Vec<i64> = nodes.iter().map(|node| node.id).collect();
    let id_map = IdMap::from_ids(&ids)
        .map_err(|id| PanGraphXError::Parse(format!("Duplicate {FORMAT} node id {id}")))?;
    let node_of = |handle: u64| -> PanResult<(NodeId, Orientation)> {
        let id = (handle >> 1).cast_signed();
        let node = id_map.get(id).ok_or_else(|| {
            PanGraphXError::Parse(format!("{FORMAT} references missing node id {id}"))
        })?;
        Ok((node, (handle & 1 == 1).into()))
    };

    let edges = nodes
        .par_iter()
        .enumerate()
        .map(|(node, record)| {
            let mut edges = Vec::new();
            for (reverse, targets) in [(false, &record.right), (true, &record.left)] {
                let from = record.id.cast_unsigned() << 1 | u64::from(reverse);
                for &to in targets {
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
        })
        .collect::<PanResult<Vec<Vec<Edge>>>>()?
        .into_iter()
        .flatten()
        .collect();

    let paths = paths
        .into_par_iter()
        .map(|path| {
            if path.circular {
                warn!(
                    "{FORMAT} path {} is circular; reading it as linear",
                    String::from_utf8_lossy(&path.name)
                );
            }
            let steps = path
                .steps
                .iter()
                .map(|&handle| {
                    let (node, orientation) = node_of(handle)?;
                    Step::new(node, orientation)
                })
                .collect::<PanResult<Vec<Step>>>()?;
            Ok(Path {
                name: path.name,
                steps,
                overlaps: Vec::new(),
            })
        })
        .collect::<PanResult<Vec<Path>>>()?;

    let node_name_map: HashMap<NodeId, Vec<u8>> = ids
        .iter()
        .enumerate()
        .map(|(node, &id)| (node, itoa::Buffer::new().format(id).as_bytes().to_vec()))
        .collect();
    Ok(CoreGraphDTO {
        nodes: Nodes::from_seq(nodes.into_iter().map(|node| node.sequence).collect()),
        edges,
        paths,
        node_name_map: Some(node_name_map),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn push(buf: &mut Vec<u8>, value: i64) {
        buf.extend_from_slice(&value.to_be_bytes());
    }

    /// A `HashGraph` of `3: ACG`, `1: T`, edges `1+ -> 3-` and `3+ -> 3-`, and
    /// path `p = 3+ 1+ 3-`, laid out the way libbdsg writes it.
    fn sample() -> Vec<u8> {
        let mut buf = HASH_GRAPH_MAGIC.to_be_bytes().to_vec();
        for value in [3, 1, 1, 2] {
            push(&mut buf, value); // max id, min id, next path id, node count
        }
        // Node 3: no left edges; right edges [flip(1+) = 1-, 3- (reversing)],
        // as both edges enter or leave its end.
        push(&mut buf, 3);
        push(&mut buf, 3);
        buf.extend_from_slice(b"ACG");
        for value in [0, 2, 1 << 1 | 1, 3 << 1 | 1] {
            push(&mut buf, value);
        }
        // Node 1: no left edges, right edges [3-].
        push(&mut buf, 1);
        push(&mut buf, 1);
        buf.push(b'T');
        for value in [0, 1, 3 << 1 | 1] {
            push(&mut buf, value);
        }
        push(&mut buf, 1); // path count
        buf.push(0);
        push(&mut buf, 0);
        push(&mut buf, 1);
        buf.push(b'p');
        for value in [3, 3 << 1, 1 << 1, 3 << 1 | 1] {
            push(&mut buf, value);
        }
        buf
    }

    #[test]
    fn reads_nodes_edges_and_paths() {
        let graph = read_hash_graph(&mut Cursor::new(sample())).unwrap();
        let sequences: Vec<&[u8]> = graph.nodes.iter().map(|n| n.sequence.as_slice()).collect();
        assert_eq!(sequences, [b"T".as_slice(), b"ACG".as_slice()]);
        assert_eq!(graph.node_name_map.as_ref().unwrap()[&0], b"1");
        let mut edges: Vec<_> = graph
            .edges
            .iter()
            .map(|e| (e.from_node, e.from_orient, e.to_node, e.to_orient))
            .collect();
        edges.sort_by_key(|&(a, ar, b, br)| {
            (a, ar == Orientation::Reverse, b, br == Orientation::Reverse)
        });
        assert_eq!(
            edges,
            [
                (0, Orientation::Forward, 1, Orientation::Reverse),
                (1, Orientation::Forward, 1, Orientation::Reverse),
            ]
        );
        let steps: Vec<_> = graph.paths[0]
            .steps
            .iter()
            .map(|s| (s.node_id(), s.is_reverse()))
            .collect();
        assert_eq!(steps, [(1, false), (0, false), (1, true)]);
    }

    #[test]
    fn rejects_truncated_and_corrupt_input() {
        let buf = sample();
        for len in [0, 3, 20, buf.len() / 2, buf.len() - 1] {
            assert!(
                read_hash_graph(&mut Cursor::new(&buf[..len])).is_err(),
                "{len}"
            );
        }
        let mut missing = buf;
        let at = missing.len() - 8;
        missing[at..].copy_from_slice(&(9_i64 << 1).to_be_bytes());
        assert!(read_hash_graph(&mut Cursor::new(missing)).is_err());
    }
}
