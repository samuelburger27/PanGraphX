//! GFA 1.0 / 1.1 codec.
//!
//! Reading is done by the streaming two-pass parser in `reader`. Writing
//! (`writer`) emits GFA 1.0 `S`, `L` and `P` lines (walks are written as
//! paths), formatted in parallel.

mod reader;
mod writer;

use crate::core::core_types::Orientation;
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::common::names::node_names;
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek, Write};

pub struct GFACodec;

/// Implementation of `GraphParser` for GFA format; see the `reader` module for details.
impl<R: Read + Seek> GraphParser<R> for GFACodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        reader::parse(reader)
    }
}

impl GraphSerializer for GFACodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        if let Some(message) = graph.dangling_reference() {
            return Err(PanGraphXError::Serialize(message));
        }
        writer::write_gfa(graph, &node_names(graph), writer)
    }
}

/// The header line every GFA written by `PanGraphX` starts with.
pub(crate) const GFA_HEADER: &[u8] = b"H\tVN:Z:1.0\n";

#[inline]
pub(crate) const fn orientation_byte(orientation: Orientation) -> u8 {
    match orientation {
        Orientation::Forward => b'+',
        Orientation::Reverse => b'-',
    }
}
