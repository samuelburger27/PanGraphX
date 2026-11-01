//! GBZ writer: the graph and a GBWT of its paths, as GBZ version 2.

use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use gbz_construct::sequences::Sequences;
use gbz_construct::{FullPathName, GBWTBuilder, GBZ, GraphBuilder, Tags, support};
use simple_sds::serialize::SerializeVersion;
use std::collections::HashMap;
use std::io::Write;

///
/// Version 3 (zstd-compressed GBWT) is the `gbz` crate's default, but vg up to
/// at least v1.73 only reads versions 1 and 2.
pub(super) const GBZ_SERIALIZE_VERSION: usize = 2;

/// Total length (in nodes) of a batch of paths fed to the GBWT builder.
/// The builder grows this automatically if a single path exceeds it.
pub(super) const GBWT_BUFFER_SIZE: usize = 1 << 20;

/// Writes `graph` as a GBZ file.
pub(super) fn write(graph: &CoreGraphDTO, mut writer: &mut dyn Write) -> PanResult<()> {
    let gbz = build_gbz(graph)?;
    gbz.serialize_version(&mut writer, GBZ_SERIALIZE_VERSION)?;
    Ok(())
}

/// Assembles a [`GBZ`] structure from a [`CoreGraphDTO`] using the construction
/// primitives provided by the `gbz` crate.
///
/// When a node name map is present, the graph nodes are treated as segments and
/// written without further splitting (`max_length = 0`). Otherwise, or when the
/// names are just the 1-based node ids (as parsed from a GBZ without a
/// translation), nodes are written with 1-based identifiers derived from their
/// dense index and no translation is stored.
pub(super) fn build_gbz(graph: &CoreGraphDTO) -> PanResult<GBZ> {
    let mut graph_builder = GraphBuilder::new();

    if let Some(name_map) = graph
        .node_name_map()
        .as_ref()
        .filter(|map| !names_are_gbz_ids(map, graph.nodes().len()))
    {
        for (id, node) in graph.nodes().iter().enumerate() {
            let name = name_map
                .get(&id)
                .ok_or_else(|| PanGraphXError::Serialize(format!("Missing name for node {id}")))?;
            let name = std::str::from_utf8(name).map_err(|e| {
                PanGraphXError::Serialize(format!("Invalid UTF-8 segment name: {e}"))
            })?;
            graph_builder
                .translate_segment(name, &node.sequence, 0)
                .map_err(PanGraphXError::Serialize)?;
        }
    } else {
        for (id, node) in graph.nodes().iter().enumerate() {
            graph_builder
                .add_node(id + 1, &node.sequence)
                .map_err(PanGraphXError::Serialize)?;
        }
    }

    let mut gbwt_builder = GBWTBuilder::new(true, true, GBWT_BUFFER_SIZE);
    let mut encoded_path = Vec::new();
    for path in graph.paths() {
        let contig_name = std::str::from_utf8(&path.name)
            .map_err(|e| PanGraphXError::Serialize(format!("Invalid UTF-8 path name: {e}")))?;
        encoded_path.clear();
        for step in &path.steps {
            encoded_path.push(support::encode_node(
                step.node_id() + 1,
                step.orientation().into(),
            ));
        }
        gbwt_builder
            .insert(&encoded_path, Some(FullPathName::generic(contig_name)))
            .map_err(PanGraphXError::Serialize)?;
    }
    let gbwt = gbwt_builder.build().map_err(PanGraphXError::Serialize)?;

    let sequences =
        Sequences::from_graph(&gbwt, &graph_builder).map_err(PanGraphXError::Serialize)?;

    GBZ::from_parts(gbwt, sequences, Tags::new()).map_err(PanGraphXError::Serialize)
}

/// Whether node `id` is named `id + 1` for every node, i.e. a translation would be redundant.
pub(super) fn names_are_gbz_ids(name_map: &HashMap<usize, Vec<u8>>, node_count: usize) -> bool {
    let mut buf = itoa::Buffer::new();
    (0..node_count).all(|id| {
        name_map
            .get(&id)
            .is_some_and(|name| name.as_slice() == buf.format(id + 1).as_bytes())
    })
}
