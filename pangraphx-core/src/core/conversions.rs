use crate::core::graph::{CoreGraph, Edge, Node, NodeId, Orientation, Path, Step};
use gfa::gfa::GFA;
use gfa::optfields::OptFields;
use std::collections::HashMap;

/// Convert a `GFA<usize, T>` (from the `gfa` parser crate) into the
/// library's internal `CoreGraph` representation.
///
/// Mapping summary:
/// - GFA segments -> `CoreGraph::nodes` (HashMap keyed by `NodeId`). The
///   GFA segment numeric `name` is used as the node id (cast to `NodeId`) and
///   the segment sequence becomes the node's sequence.
/// - GFA links -> `CoreGraph::edges`. Orientation and overlap information is
///   converted using `Orientation::from_gfa_lib`.
/// - GFA paths -> `CoreGraph::paths`. Each GFA path becomes a `Path` with a
///   sequence of `Step`s; the path name (`Vec<u8>`) is used as the key.
///
/// Notes and assumptions:
/// - This conversion consumes the `GFA` instance (ownership is moved).
/// - It assumes segment names are numeric (`usize`) and uniquely identify
///   segments; they are cast to `NodeId` (`u64`). If your GFA uses non-numeric
///   identifiers, adjust parsing or conversion accordingly.
/// - Optional GFA fields (optfields) are currently ignored.
impl<T: OptFields> From<GFA<usize, T>> for CoreGraph {
    fn from(gfa: GFA<usize, T>) -> Self {
        // TODO handle options fields properly
        // TODO handle ID types other than usize
        let nodes: HashMap<NodeId, Node> = gfa
            .segments
            .into_iter()
            .map(|seq| {
                let node_id = seq.name as NodeId;
                let sequence = seq.sequence;
                (
                    node_id,
                    Node {
                        id: node_id,
                        sequence,
                    },
                )
            })
            .collect();
        let edges: Vec<Edge> = gfa
            .links
            .into_iter()
            .map(|link| Edge {
                from_node: link.from_segment as NodeId,
                from_orient: Orientation::from_gfa_lib(link.from_orient),
                to_node: link.to_segment as NodeId,
                to_orient: Orientation::from_gfa_lib(link.to_orient),
                overlap: link.overlap,
            })
            .collect();

        let paths: HashMap<Vec<u8>, Path> = gfa
            .paths
            .into_iter()
            .map(|p| {
                let steps: Vec<Step> = p
                    .iter()
                    .map(|(id, orient)| Step {
                        node_id: id as u64,
                        orientation: Orientation::from_gfa_lib(orient),
                    })
                    .collect();
                let name = p.path_name;
                return (
                    name.clone(),
                    Path {
                        name,
                        steps,
                        overlaps: p.overlaps,
                    },
                );
            })
            .collect();

        CoreGraph {
            nodes,
            edges,
            paths,
        }
    }
}
