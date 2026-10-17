use crate::core::core_types::{Edge, Node, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::gfa_format::GFACodec;
use crate::traits::{GraphParser, GraphSerializer};
use odgi_ffi::{Edge as OdgiEdge, Graph as OdgiGraph, gfa_to_odgi};
use std::io::{Read, Seek, Write, copy};
use tempfile::NamedTempFile;

/// Codec for reading and writing ODGI graphs.
///
/// The current FFI layer accepts filesystem paths rather than generic readers/writers,
/// so parsing and serialization use temporary files as an interoperability bridge.
pub struct ODGICodec;

/// Converts an `odgi_ffi` orientation flag, which is `true` for forward, to an [`Orientation`].
const fn orientation_from_is_forward(is_forward: bool) -> Orientation {
    if is_forward {
        Orientation::Forward
    } else {
        Orientation::Reverse
    }
}

/// Converts an ODGI edge to the internal `Edge` representation.
///
/// `from_id` is a 0-based index into the node vector, while `edge.to_node`
/// is the 1-based ODGI node identifier, so it is shifted by one.
#[allow(clippy::cast_possible_truncation)]
impl From<(usize, OdgiEdge)> for Edge {
    fn from((from_id, edge): (usize, OdgiEdge)) -> Self {
        Self {
            from_node: from_id,
            from_orient: orientation_from_is_forward(edge.from_orientation),
            to_node: (edge.to_node - 1) as usize,
            to_orient: orientation_from_is_forward(edge.to_orientation),
            overlap: 0, // ODGI edges do not have explicit overlap information
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
impl<R: Read + Seek> GraphParser<R> for ODGICodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        // `odgi_ffi` currently parses from a file path, so the input stream is staged
        // into a temporary file before loading.

        let mut temp_file = NamedTempFile::new()?;
        copy(reader, &mut temp_file)?;
        let temp_path = temp_file.path().to_str().ok_or_else(|| {
            PanGraphXError::Parse("Failed to convert temp file path to string".to_string())
        })?;

        let odgi_graph = OdgiGraph::load(temp_path)
            .map_err(|e| PanGraphXError::Parse(format!("Failed to parse ODGI graph: {e:?}")))?;

        temp_file.close()?;

        let node_len = odgi_graph.node_count();

        let mut nodes = Vec::with_capacity(node_len as usize);
        let mut edges = Vec::with_capacity(node_len as usize);

        // ODGI node identifiers are 1-based, while the internal `NodeId` is a
        // 0-based index into the node vector. Iterate over the 1-based IDs and
        // convert to 0-based indices when storing.
        for odgi_id in 1..=node_len {
            let seq = odgi_graph.get_node_sequence(odgi_id).into_bytes();

            nodes.push(Node {
                id: (odgi_id - 1) as usize,
                sequence: seq,
            });

            let successors = odgi_graph.get_successors(odgi_id);
            for succ in successors {
                edges.push(Edge::from(((odgi_id - 1) as usize, succ)));
            }
        }
        let path_names = odgi_graph.get_path_names();
        let mut paths = Vec::with_capacity(path_names.len());

        // Build each path by projecting every position to its corresponding node/orientation.
        for path_name in path_names {
            let path_len = odgi_graph.get_path_length(&path_name).ok_or_else(|| {
                PanGraphXError::Parse(format!("Failed to get path length for path: {path_name}"))
            })?;
            let mut steps = Vec::with_capacity(path_len as usize);
            for pos in 0..path_len {
                let path_pos = odgi_graph.project(&path_name, pos).ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "Failed to project path position for path: {path_name}, pos: {pos}"
                    ))
                })?;
                steps.push(Step::new(
                    (path_pos.node_id - 1) as usize,
                    orientation_from_is_forward(path_pos.is_forward),
                )?);
            }
            paths.push(Path {
                name: path_name.into_bytes(),
                steps,
                overlaps: Vec::new(), // ODGI does not expose explicit overlap values.
            });
        }

        let nodes = Nodes::from_node_vec(nodes)?;

        Ok(CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map: None, // ODGI does not expose a dedicated node name map.
        })
    }
}

impl GraphSerializer for ODGICodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        // `gfa_to_odgi` converts between files, so serialize to temporary GFA first,
        // convert to temporary ODGI, then stream the result to the destination writer.
        let mut temp_file = NamedTempFile::new()?;
        let temp_path = temp_file.path().to_string_lossy().to_string();
        let gfa_codec = GFACodec;
        gfa_codec.serialize(graph, &mut temp_file)?;
        temp_file.flush()?;

        let mut output_file = NamedTempFile::new()?;
        let output_path = output_file.path().to_string_lossy().to_string();

        gfa_to_odgi(&temp_path, &output_path).map_err(|e| {
            PanGraphXError::Serialize(format!("Failed to convert GFA to ODGI format: {e:?}"))
        })?;
        copy(&mut output_file, writer)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_keeps_orientations() {
        let original = CoreGraphDTO {
            nodes: Nodes::from_seq(vec![b"ACGT".to_vec(), b"GGA".to_vec()]),
            edges: vec![Edge {
                from_node: 0,
                from_orient: Orientation::Forward,
                to_node: 1,
                to_orient: Orientation::Reverse,
                overlap: 0,
            }],
            paths: vec![Path {
                name: b"p".to_vec(),
                steps: vec![
                    Step::new(0, Orientation::Forward).unwrap(),
                    Step::new(1, Orientation::Reverse).unwrap(),
                ],
                overlaps: Vec::new(),
            }],
            node_name_map: None,
        };

        let mut buf = Vec::new();
        ODGICodec.serialize(&original, &mut buf).unwrap();
        let parsed = ODGICodec.parse(&mut Cursor::new(buf)).unwrap();

        let steps: Vec<_> = parsed.paths[0]
            .steps
            .iter()
            .map(|s| (s.node_id(), s.orientation()))
            .collect();
        assert_eq!(
            steps,
            vec![(0, Orientation::Forward), (1, Orientation::Reverse)]
        );
        // The link is reported from both of its ends: 1+ -> 2- and 2+ -> 1-.
        let edges: Vec<_> = parsed
            .edges
            .iter()
            .map(|e| (e.from_node, e.from_orient, e.to_node, e.to_orient))
            .collect();
        assert!(edges.contains(&(0, Orientation::Forward, 1, Orientation::Reverse)));
        assert!(!edges.contains(&(0, Orientation::Reverse, 1, Orientation::Forward)));
    }
}
