//! VG codec: the protobuf `Graph` messages of `vg`, in libvgio framing.
//!
//! - `framing`: varints and the group / length-prefixed message framing.
//! - `reader`: decodes message windows in parallel and merges them in stream order.
//! - `writer`: splits the graph into `Graph` chunks and encodes them in parallel.
//!
//! Node ids are kept when every name is a distinct, positive, canonically
//! written integer; otherwise nodes are numbered `1..=n` (names go to the VG
//! `name` field). Per-step path overlaps are not part of the format.

mod framing;
mod reader;
mod writer;

#[cfg(test)]
mod tests;

use crate::core::core_types::Orientation;
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::PanResult;
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek, Write};

/// Codec for vg's protobuf graph format (`.vg`).
pub struct VGCodec;

impl<R: Read + Seek> GraphParser<R> for VGCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        reader::parse_vg(reader, reader::VG_READ_WINDOW)
    }
}

impl GraphSerializer for VGCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        writer::write_vg(graph, writer)
    }
}

/// VG Edge orientation convention:
/// - `from_start = false` means leaving from the end of the node → Forward
/// - `from_start = true` means leaving from the start of the node → Reverse
/// - `to_end = false` means arriving at the start of the node → Forward
/// - `to_end = true` means arriving at the end of the node → Reverse
pub(super) const fn vg_edge_to_orientations(
    from_start: bool,
    to_end: bool,
) -> (Orientation, Orientation) {
    let from_orient = if from_start {
        Orientation::Reverse
    } else {
        Orientation::Forward
    };
    let to_orient = if to_end {
        Orientation::Reverse
    } else {
        Orientation::Forward
    };
    (from_orient, to_orient)
}

pub(super) fn orientations_to_vg_edge(
    from_orient: Orientation,
    to_orient: Orientation,
) -> (bool, bool) {
    let from_start = from_orient == Orientation::Reverse;
    let to_end = to_orient == Orientation::Reverse;
    (from_start, to_end)
}
