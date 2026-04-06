use crate::core::core_types::{Edge, Node, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use std::collections::HashMap;
use std::io::{Read, Seek};

// Import necessary types from the gbwt crate
use gbwt::{GBZ, Orientation as GbwtOrientation, REF_SAMPLE};
use simple_sds_sbwt::serialize::Serialize;

pub struct GBZCodec;

// Helper to map GBWT orientation to internal CoreGraph orientation
impl From<GbwtOrientation> for Orientation {
    fn from(o: GbwtOrientation) -> Self {
        match o {
            GbwtOrientation::Forward => Orientation::Forward,
            GbwtOrientation::Reverse => Orientation::Reverse,
        }
    }
}
// TODO should rewrite

impl<R: Read + Seek> GraphParser<R> for GBZCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        // 1. Load the GBZ file using the Serialize trait method
        let gbz = GBZ::load(reader)?;

        let Some(metadata) = gbz.metadata() else {
            return Err(PanGraphXError::Parse(
                "GBZ file missing metadata".to_string(),
            ));
        };

        let node_name_map;

        let mut node_seq = vec![Node::default(); gbz.nodes()];
        // 1. Load Nodes
        match gbz.segment_iter() {
            Some(iter) => {
                let mut map = HashMap::new();
                for segment in iter {
                    let id = segment.id;
                    map.insert(id, segment.name.to_vec());
                    node_seq[id] = Node {
                        id,
                        sequence: segment.sequence.to_vec(),
                    };
                }
                node_name_map = Some(map);
            }
            None => {
                node_name_map = None; // No segment names, disable node name mapping
                for node_id in gbz.node_iter() {
                    let id = node_id - 1; // GBZ node IDs are 1-based, convert to 0-based
                    let sequence = gbz
                        .sequence(node_id)
                        .ok_or_else(|| {
                            PanGraphXError::Parse(format!(
                                "Node {} doesn't have a sequence, GBZ file is not valid",
                                node_id
                            ))
                        })?
                        .to_vec();
                    node_seq[id] = Node {
                        id,
                        sequence: sequence.clone(),
                    };
                }
            }
        }

        // 2. Load Edges
        // TODO this can be refactored probably
        let mut edges = Vec::new();
        match gbz.segment_iter() {
            Some(iter) => {
                for segment in iter {
                    for (successor, orientation) in gbz
                        .segment_successors(&segment, GbwtOrientation::Forward)
                        .unwrap()
                    {
                        // A link from forward orientation is canonical if it is a self-loop or the successor
                        // has a greater id.
                        if successor.id >= segment.id {
                            edges.push(Edge {
                                from_node: segment.id,
                                from_orient: Orientation::Forward,
                                to_node: successor.id,
                                to_orient: orientation.into(),
                                overlap: 0, // GBZ does not store overlap information TODO check, workaround ?
                            });
                        }
                    }
                    for (successor, orientation) in gbz
                        .segment_successors(&segment, GbwtOrientation::Reverse)
                        .unwrap()
                    {
                        // A link from reverse orientation is canonical if it is a self-loop that to forward
                        // orientation or the successor has a greater id.
                        if successor.id > segment.id
                            || (successor.id == segment.id
                                && orientation == GbwtOrientation::Forward)
                        {
                            edges.push(Edge {
                                from_node: segment.id, // Convert to 0-based ID
                                from_orient: Orientation::Reverse,
                                to_node: successor.id, // Convert to 0-based ID
                                to_orient: orientation.into(),
                                overlap: 0, // GBZ does not store overlap information TODO check, workaround ?
                            });
                        }
                    }
                }
            }
            None => {
                for node_id in gbz.node_iter() {
                    for (successor, orientation) in
                        gbz.successors(node_id, GbwtOrientation::Forward).unwrap()
                    {
                        if successor >= node_id {
                            edges.push(Edge {
                                from_node: node_id - 1, // Convert to 0-based ID
                                from_orient: Orientation::Forward,
                                to_node: successor - 1, // Convert to 0-based ID
                                to_orient: orientation.into(),
                                overlap: 0, // GBZ does not store overlap information TODO check, workaround ?
                            });
                        }
                    }
                    for (successor, orientation) in
                        gbz.successors(node_id, GbwtOrientation::Reverse).unwrap()
                    {
                        if successor > node_id
                            || (successor == node_id && orientation == GbwtOrientation::Forward)
                        {
                            edges.push(Edge {
                                from_node: node_id - 1, // Convert to 0-based ID
                                from_orient: Orientation::Reverse,
                                to_node: successor - 1, // Convert to 0-based ID
                                to_orient: orientation.into(),
                                overlap: 0, // GBZ does not store overlap information TODO check, workaround ?
                            });
                        }
                    }
                }
            }
        }

        // 3. Load Paths
        let mut paths = Vec::new();
        let ref_sample = metadata
            .sample_id(REF_SAMPLE)
            .ok_or_else(|| PanGraphXError::Parse("GBZ file missing path names".to_string()))?;

        for (path_id, path_name) in metadata.path_iter().enumerate() {
            if path_name.sample() == ref_sample {
                let contig_name = metadata.contig(path_name.contig()).ok_or_else(|| {
                    PanGraphXError::Parse("GBZ file missing contig name".to_string())
                })?;
                let steps: Vec<Step> = match gbz.segment_path(path_id, GbwtOrientation::Forward) {
                    Some(iter) => iter
                        .map(|(segment, orientation)| Step {
                            node_id: segment.id,
                            orientation: orientation.into(),
                        })
                        .collect(),

                    None => gbz
                        .path(path_id, GbwtOrientation::Forward)
                        .ok_or_else(|| {
                            PanGraphXError::Parse("GBZ file missing path steps".to_string())
                        })?
                        .map(|(node_id, orientation)| Step {
                            node_id: node_id - 1, // Convert to 0-based ID
                            orientation: orientation.into(),
                        })
                        .collect(),
                };
                let overlaps: Vec<u32> = vec![0; steps.len().saturating_sub(1)]; // GBZ does not store overlaps, default to 0
                paths.push(Path {
                    name: contig_name.as_bytes().to_vec(),
                    steps,
                    overlaps,
                });
            }
        }
        let nodes = Nodes::from_node_vec(node_seq)?;

        Ok(CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map,
        })
    }
}

impl GraphSerializer for GBZCodec {
    fn serialize(&self, _graph: &CoreGraphDTO, _writer: &mut dyn std::io::Write) -> PanResult<()> {
        todo!("GBZ format serializer not implemented yet")
    }
}
