use crate::core::core_types::{Edge, Node, Orientation, Path};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::gfa_parser;
use crate::traits::{GraphParser, GraphSerializer};
use rayon::prelude::*;
use std::borrow::Cow;
use std::io::{Read, Seek, Write};

pub struct GFACodec;

/// Implementation of `GraphParser` for GFA format; see the `gfa_parser` module for details.
impl<R: Read + Seek> GraphParser<R> for GFACodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        gfa_parser::parse(reader)
    }
}

/// Number of S/L records formatted per parallel task.
pub(crate) const RECORDS_PER_CHUNK: usize = 1 << 14;

/// Builds the id -> name table once, so formatting never hashes or allocates per step.
/// Nodes without an entry in `node_name_map` are named by their numeric id.
pub(crate) fn node_names(graph: &CoreGraphDTO) -> Vec<Cow<'_, [u8]>> {
    (0..graph.nodes.len())
        .into_par_iter()
        .map(|id| {
            graph
                .node_name_map
                .as_ref()
                .and_then(|map| map.get(&id))
                .map_or_else(
                    || Cow::Owned(itoa::Buffer::new().format(id).as_bytes().to_vec()),
                    |name| Cow::Borrowed(name.as_slice()),
                )
        })
        .collect()
}

/// Names every node by its 1-based id, ignoring `node_name_map`.
pub(crate) fn one_based_names(graph: &CoreGraphDTO) -> Vec<Cow<'_, [u8]>> {
    (1..=graph.nodes.len())
        .into_par_iter()
        .map(|id| Cow::Owned(itoa::Buffer::new().format(id).as_bytes().to_vec()))
        .collect()
}

#[inline]
const fn orientation_byte(orientation: Orientation) -> u8 {
    match orientation {
        Orientation::Forward => b'+',
        Orientation::Reverse => b'-',
    }
}

/// Formats `items` in parallel and writes the result in order.
///
/// Only one window of chunks (`RECORDS_PER_CHUNK` items per thread) is held in
/// memory at a time, so the output text is never fully resident.
pub(crate) fn write_records<T, F>(
    writer: &mut dyn Write,
    items: &[T],
    per_chunk: usize,
    format: F,
) -> PanResult<()>
where
    T: Sync,
    F: Fn(&T, &mut Vec<u8>) + Sync,
{
    let window = per_chunk * rayon::current_num_threads();
    for window_items in items.chunks(window) {
        let buffers: Vec<Vec<u8>> = window_items
            .par_chunks(per_chunk)
            .map(|chunk| {
                let mut buf = Vec::new();
                for item in chunk {
                    format(item, &mut buf);
                }
                buf
            })
            .collect();
        for buf in buffers {
            writer.write_all(&buf)?;
        }
    }
    Ok(())
}

fn format_segment(names: &[Cow<'_, [u8]>], node: &Node, buf: &mut Vec<u8>) {
    buf.extend_from_slice(b"S\t");
    buf.extend_from_slice(&names[node.id]);
    buf.push(b'\t');
    buf.extend_from_slice(&node.sequence);
    buf.push(b'\n');
}

fn format_link(names: &[Cow<'_, [u8]>], edge: &Edge, buf: &mut Vec<u8>) {
    buf.extend_from_slice(b"L\t");
    buf.extend_from_slice(&names[edge.from_node]);
    buf.push(b'\t');
    buf.push(orientation_byte(edge.from_orient));
    buf.push(b'\t');
    buf.extend_from_slice(&names[edge.to_node]);
    buf.push(b'\t');
    buf.push(orientation_byte(edge.to_orient));
    buf.push(b'\t');
    buf.extend_from_slice(itoa::Buffer::new().format(edge.overlap).as_bytes());
    buf.extend_from_slice(b"M\n");
}

fn format_path(names: &[Cow<'_, [u8]>], path: &Path, buf: &mut Vec<u8>) {
    buf.extend_from_slice(b"P\t");
    buf.extend_from_slice(&path.name);
    buf.push(b'\t');
    for (i, step) in path.steps.iter().enumerate() {
        if i > 0 {
            buf.push(b',');
        }
        buf.extend_from_slice(&names[step.node_id()]);
        buf.push(orientation_byte(step.orientation()));
    }
    buf.push(b'\t');
    // All-zero overlaps carry no information, so they are written as `*`.
    if path.overlaps.iter().all(|&overlap| overlap == 0) {
        buf.push(b'*');
    } else {
        let mut itoa_buf = itoa::Buffer::new();
        for (i, &overlap) in path.overlaps.iter().enumerate() {
            if i > 0 {
                buf.push(b',');
            }
            buf.extend_from_slice(itoa_buf.format(overlap).as_bytes());
            buf.push(b'M');
        }
    }
    buf.push(b'\n');
}

impl GraphSerializer for GFACodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        if let Some(message) = graph.dangling_reference() {
            return Err(PanGraphXError::Serialize(message));
        }
        write_gfa(graph, &node_names(graph), writer)
    }
}

fn write_gfa(
    graph: &CoreGraphDTO,
    names: &[Cow<'_, [u8]>],
    writer: &mut dyn Write,
) -> PanResult<()> {
    writer.write_all(b"H\tVN:Z:1.0\n")?;
    write_records(
        writer,
        graph.nodes.as_slice(),
        RECORDS_PER_CHUNK,
        |node, buf| {
            format_segment(names, node, buf);
        },
    )?;
    write_records(writer, &graph.edges, RECORDS_PER_CHUNK, |edge, buf| {
        format_link(names, edge, buf);
    })?;
    // A single path line can be hundreds of MB, so each path is its own task.
    write_records(writer, &graph.paths, 1, |path, buf| {
        format_path(names, path, buf);
    })?;
    Ok(())
}
