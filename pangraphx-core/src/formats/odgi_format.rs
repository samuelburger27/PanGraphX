use crate::core::graph_dto::{CoreGraphDTO, Edge, Node, NodeId, Orientation, Path, Step};
use crate::formats::gfa_format::GFACodec;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use odgi_ffi::{Edge as OdgiEdge, Error, Graph as OdgiGraph, gfa_to_odgi};
use std::io::{BufRead, BufReader, Read, Seek, Write, copy};
use tempfile::NamedTempFile;

pub struct ODGICodec;

/// Converts an ODGI edge to our internal Edge representation
impl From<(usize, OdgiEdge)> for Edge {
    fn from((from_id, edge): (usize, OdgiEdge)) -> Self {
        Edge {
            from_node: from_id,
            from_orient: Orientation::from(edge.from_orientation),
            to_node: edge.to_node as usize,
            to_orient: Orientation::from(edge.to_orientation),
            // TODO check
            overlap: 0, // ODGI edges do not have explicit overlap information, so we set it to 0 for now
        }
    }
}

impl<R: Read + Seek> GraphParser<R> for ODGICodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        // This is problematic, as odgi_ffi expects a file path, but we want to read from a stream
        // Workaround is to read the entire stream into a temporary file, but this is not ideal

        let mut temp_file = NamedTempFile::new()?;
        copy(reader, &mut temp_file)?;
        let temp_path = temp_file.path().to_str().ok_or_else(|| {
            PanGraphXError::Other("Failed to convert temp file path to string".to_string())
        })?;
        let odgi_graph = OdgiGraph::load(temp_path)
            .map_err(|e| PanGraphXError::Parse(format!("Failed to parse ODGI graph: {:?}", e)))?;

        let node_len = odgi_graph.node_count() as usize;

        let mut nodes = Vec::with_capacity(node_len);
        let mut edges = Vec::with_capacity(node_len);

        for node_id in 0..node_len {
            let seq = odgi_graph.get_node_sequence(node_id as u64).into_bytes();

            nodes.push(Node {
                id: node_id,
                sequence: seq,
            });

            let successors = odgi_graph.get_successors(node_id as u64);
            for succ in successors {
                edges.push(Edge::from((node_id, succ)));
            }
        }
        let path_names = odgi_graph.get_path_names();
        let mut paths = Vec::with_capacity(path_names.len());
        // TODO test this not sure if i understand the API correctly
        for path_name in path_names {
            let path_len = odgi_graph.get_path_length(&path_name).ok_or_else(|| {
                PanGraphXError::Parse(format!("Failed to get path length for path: {}", path_name))
            })?;
            let mut steps = Vec::with_capacity(path_len as usize);
            for pos in 0..path_len {
                let path_pos = odgi_graph.project(&path_name, pos).ok_or_else(|| {
                    PanGraphXError::Parse(format!(
                        "Failed to project path position for path: {}, pos: {}",
                        path_name, pos
                    ))
                })?;
                steps.push(Step {
                    node_id: path_pos.node_id as usize,
                    orientation: Orientation::from(path_pos.is_forward),
                });
            }
            paths.push(Path {
                name: path_name.into_bytes(),
                steps,
                overlaps: Vec::new(), // ODGI does not have explicit overlap information, so we set it to an empty vector for now
            });
        }

        Ok(CoreGraphDTO {
            nodes,
            edges,
            paths,
            node_name_map: None, // ODGI does not have explicit node names, so we set this to None for now
        })
    }
}

impl GraphSerializer for ODGICodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        // Once again this is ugly, we need to write to a temporary gfa file and then convert it to odgi format using the gfa_to_odgi function from the odgi_ffi crate, 
        // which expects file paths, and then write the resulting odgi file to the output writer
        let mut temp_file = NamedTempFile::new()?;
        let temp_path = temp_file.path().to_string_lossy().to_string();
        let gfa_codec = GFACodec;
        gfa_codec.serialize(graph, &mut temp_file)?;

        let mut output_file = NamedTempFile::new()?;

        gfa_to_odgi(&temp_path, output_file.path().to_str().unwrap()).map_err(|e| {
            PanGraphXError::Parse(format!("Failed to convert GFA to ODGI format: {:?}", e))
        })?;
        copy(&mut output_file, writer)?;
        Ok(())
    }
}
