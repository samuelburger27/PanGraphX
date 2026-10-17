use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::gfa_format::{self, GFACodec};
use crate::traits::{GraphParser, GraphSerializer};
use odgi_ffi::{gfa_to_odgi, odgi_to_gfa};
use std::fs::File;
use std::io::{Read, Seek, Write, copy};
use tempfile::NamedTempFile;

/// Codec for reading and writing ODGI graphs.
///
/// odgi only works on files, so parsing and serialization go through temporary
/// files and GFA: `odgi_ffi` runs `odgi view` / `odgi build` to convert, and the
/// GFA codec does the rest.
///
/// # Performance
/// ODGI support is slow and memory hungry, and is not meant for large graphs.
/// Every conversion runs the external `odgi` executable and writes the whole
/// graph to temporary files as both ODGI and GFA. On top of that, reading
/// (`odgi_ffi::odgi_to_gfa`) holds the complete GFA text output by `odgi view`
/// in memory before writing it out, so peak memory is the size of the GFA text
/// plus the parsed graph.
pub struct ODGICodec;

impl<R: Read + Seek> GraphParser<R> for ODGICodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        // `odgi_to_gfa` converts between files, so stage the input as a temporary
        // ODGI file, convert it to temporary GFA, then parse that with the GFA codec.
        let mut odgi_file = NamedTempFile::new()?;
        copy(reader, &mut odgi_file)?;
        odgi_file.flush()?;
        let odgi_path = odgi_file.path().to_string_lossy().to_string();

        let gfa_file = NamedTempFile::new()?;
        let gfa_path = gfa_file.path().to_string_lossy().to_string();

        odgi_to_gfa(&odgi_path, &gfa_path).map_err(|e| {
            PanGraphXError::Parse(format!("Failed to convert ODGI to GFA format: {e:?}"))
        })?;
        drop(odgi_file);

        GFACodec.parse(&mut File::open(gfa_file.path())?)
    }
}

impl GraphSerializer for ODGICodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        // `gfa_to_odgi` converts between files, so serialize to temporary GFA first,
        // convert to temporary ODGI, then stream the result to the destination writer.
        // odgi needs positive integer node ids, so segments are named by 1-based node id.
        let mut temp_file = NamedTempFile::new()?;
        let temp_path = temp_file.path().to_string_lossy().to_string();
        gfa_format::serialize_with_one_based_ids(graph, &mut temp_file)?;
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
    use crate::core::core_types::{Edge, Nodes, Orientation, Path, Step};
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

        // Compare by segment name (`1..=n`, as written for odgi), not by node order.
        let name = |id| parsed.get_name_from_id(id);
        let steps: Vec<_> = parsed.paths[0]
            .steps
            .iter()
            .map(|s| (name(s.node_id()), s.orientation()))
            .collect();
        assert_eq!(
            steps,
            vec![
                ("1".to_string(), Orientation::Forward),
                ("2".to_string(), Orientation::Reverse)
            ]
        );
        // One edge per link; odgi may write it from either end.
        let edges: Vec<_> = parsed
            .edges
            .iter()
            .map(|e| {
                (
                    name(e.from_node),
                    e.from_orient,
                    name(e.to_node),
                    e.to_orient,
                )
            })
            .collect();
        let forward = (
            "1".to_string(),
            Orientation::Forward,
            "2".to_string(),
            Orientation::Reverse,
        );
        let flipped = (
            "2".to_string(),
            Orientation::Forward,
            "1".to_string(),
            Orientation::Reverse,
        );
        assert!(edges == [forward] || edges == [flipped], "{edges:?}");
    }
}
