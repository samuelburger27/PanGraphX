//! GBZ codec, built on the `gbz-construct` crate.
//!
//! `reader` loads a GBZ and converts its segments (or nodes), the canonical
//! edges and the paths with their metadata names. `writer` builds the GBWT of
//! the paths and writes GBZ version 2, which vg can read. Only what the paths
//! imply is kept: no overlaps, no off-path or repeated edges.

mod reader;
mod writer;

#[cfg(test)]
mod tests;

use crate::core::core_types::Orientation;
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::PanResult;
use crate::traits::{GraphParser, GraphSerializer};
use gbz_construct::Orientation as GbzOrientation;
use std::io::{Read, Seek, Write};

/// Codec for the GBZ format (GBWT paths plus the node sequences).
pub struct GBZCodec;

impl<R: Read + Seek> GraphParser<R> for GBZCodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        reader::parse(reader)
    }
}

impl GraphSerializer for GBZCodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        writer::write(graph, writer)
    }
}

impl From<GbzOrientation> for Orientation {
    fn from(o: GbzOrientation) -> Self {
        match o {
            GbzOrientation::Forward => Self::Forward,
            GbzOrientation::Reverse => Self::Reverse,
        }
    }
}

impl From<Orientation> for GbzOrientation {
    fn from(o: Orientation) -> Self {
        match o {
            Orientation::Forward => Self::Forward,
            Orientation::Reverse => Self::Reverse,
        }
    }
}
