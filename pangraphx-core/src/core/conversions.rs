use crate::core::graph::{CoreGraph, Edge, Node, NodeId, Orientation, Path, Step};
use gfa::gfa::GFA;
use gfa::optfields::OptFields;

/// Convert a `GFA<Vec<u8>, T>` (from the `gfa` parser crate) into the
/// library's internal `CoreGraph` representation.
/// Notes and assumptions:
/// - This conversion consumes the `GFA` instance (ownership is moved).
/// - Optional GFA fields (optfields) are currently ignored.
impl<T: OptFields> From<GFA<Vec<u8>, T>> for CoreGraph {
    fn from(gfa: GFA<Vec<u8>, T>) -> Self {
        // TODO handle options fields properly
        let nodes: Vec<Node> = gfa
            .segments
            .into_iter()
            .map(|seq| Node {
                id: seq.name,
                sequence: seq.sequence,
            })
            .collect();
        let edges: Vec<Edge> = gfa
            .links
            .into_iter()
            .map(|link| Edge {
                from_node: link.from_segment as NodeId,
                from_orient: Orientation::from_gfa(link.from_orient),
                to_node: link.to_segment as NodeId,
                to_orient: Orientation::from_gfa(link.to_orient),
                overlap: link.overlap,
            })
            .collect();

        let paths: Vec<Path> = gfa
            .paths
            .into_iter()
            .map(|p| {
                let steps: Vec<Step> = p
                    .iter()
                    .map(|(id, orient)| Step {
                        node_id: id.to_vec(),
                        orientation: Orientation::from_gfa(orient),
                    })
                    .collect();
                Path {
                    name: p.path_name,
                    steps,
                    overlaps: p.overlaps,
                }
            })
            .collect();

        CoreGraph {
            nodes,
            edges,
            paths,
        }
    }
}
