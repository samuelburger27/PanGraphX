use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek, Write};

/// Codec for exporting graphs to Graphviz DOT format.
///
/// DOT is a text-based graph description language used by the Graphviz tool
/// suite for visualization. This codec is export-only: it serializes the nodes
/// and directed edges of a graph, but does not support parsing DOT back into a
/// [`CoreGraphDTO`].
///
/// Each node is emitted as a single DOT node labeled with its original name.
/// Edges are directed from `from_node` to `to_node` and carry a label encoding
/// the traversal orientation at each end (e.g. `+/+`, `+/-`, `-/+`, `-/-`).
pub struct DOTCodec;

/// Quotes a DOT identifier/label and escapes any embedded quotes or backslashes.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

impl GraphSerializer for DOTCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        writer.write_all(b"digraph G {\n")?;
        writer.write_all(b"\trankdir=LR;\n")?;

        for node in &graph.nodes {
            let quoted = quote(&graph.get_name_from_id(node.id));
            writer.write_all(format!("\t{quoted} [label={quoted}];\n").as_bytes())?;
        }

        for edge in &graph.edges {
            let from = quote(&graph.get_name_from_id(edge.from_node));
            let to = quote(&graph.get_name_from_id(edge.to_node));
            writer.write_all(
                format!(
                    "\t{from} -> {to} [label=\"{}/{}\"];\n",
                    edge.from_orient, edge.to_orient
                )
                .as_bytes(),
            )?;
        }

        writer.write_all(b"}\n")?;
        Ok(())
    }
}

impl<R: Read + Seek> GraphParser<R> for DOTCodec {
    fn parse(&self, _reader: &mut R) -> PanResult<CoreGraphDTO> {
        Err(PanGraphXError::DeserializationNotSupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::core_types::{Edge, Nodes, Orientation};
    use std::collections::HashMap;

    fn make_graph() -> CoreGraphDTO {
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
                    from_orient: Orientation::Reverse,
                    to_node: 2,
                    to_orient: Orientation::Forward,
                    overlap: 0,
                },
            ],
            paths: vec![],
            node_name_map: Some(name_map),
        }
    }

    #[test]
    fn test_serialize_basic_graph() {
        let graph = make_graph();
        let mut out = Vec::new();
        DOTCodec.serialize(&graph, &mut out).unwrap();

        let expected = "digraph G {\n\
            \trankdir=LR;\n\
            \t\"1\" [label=\"1\"];\n\
            \t\"2\" [label=\"2\"];\n\
            \t\"3\" [label=\"3\"];\n\
            \t\"1\" -> \"2\" [label=\"+/+\"];\n\
            \t\"2\" -> \"3\" [label=\"-/+\"];\n\
            }\n";
        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }

    #[test]
    fn test_serialize_empty_graph() {
        let graph = CoreGraphDTO::default();
        let mut out = Vec::new();
        DOTCodec.serialize(&graph, &mut out).unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "digraph G {\n\trankdir=LR;\n}\n"
        );
    }

    #[test]
    fn test_quote_escapes_special_characters() {
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn test_parse_returns_unsupported_error() {
        let mut input = std::io::Cursor::new(&b"digraph G {}"[..]);
        let result = DOTCodec.parse(&mut input);
        assert!(matches!(
            result,
            Err(PanGraphXError::DeserializationNotSupported)
        ));
    }
}
