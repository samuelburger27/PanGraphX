//! GFA 1.0 writer: `S`, `L` and `P` lines.

use super::{GFA_HEADER, orientation_byte};
use crate::core::core_types::{Edge, Node, Path};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::common::chunked::{RECORDS_PER_CHUNK, write_records};
use crate::formats::common::names::node_names;
use std::borrow::Cow;
use std::io::Write;

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

/// Writes `graph` as GFA 1.0, naming nodes by their original names.
pub(super) fn write(graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
    if let Some(message) = graph.dangling_reference() {
        return Err(PanGraphXError::Serialize(message));
    }
    write_gfa(graph, &node_names(graph), writer)
}

fn write_gfa(
    graph: &CoreGraphDTO,
    names: &[Cow<'_, [u8]>],
    writer: &mut dyn Write,
) -> PanResult<()> {
    writer.write_all(GFA_HEADER)?;
    write_records(
        writer,
        graph.nodes.as_slice(),
        RECORDS_PER_CHUNK,
        |_, node, buf| {
            format_segment(names, node, buf);
            Ok(())
        },
    )?;
    write_records(writer, &graph.edges, RECORDS_PER_CHUNK, |_, edge, buf| {
        format_link(names, edge, buf);
        Ok(())
    })?;
    // A single path line can be hundreds of MB, so each path is its own task.
    write_records(writer, &graph.paths, 1, |_, path, buf| {
        format_path(names, path, buf);
        Ok(())
    })?;
    Ok(())
}
