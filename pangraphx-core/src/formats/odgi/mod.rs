//! Native reader and writer for odgi's binary graph format (`.og`).
//!
//! The layout mirrors odgi's `graph_t::serialize` (libhandlegraph `Serializable`
//! with odgi's `node_t` records and DYNAMIC's `hacked_vector`). All integers are
//! little-endian except the leading magic number:
//!
//! ```text
//! u32 BE  magic (1988148666)
//! u64 ×7  max_node_id, min_node_id, node_count, edge_count, path_count,
//!         path_handle_next, id_increment
//! node_count × node record (node rank r has id r + 1 + id_increment):
//!   u64 sequence length, sequence bytes, u64 id (0 marks a deleted node)
//!   packed edges    (other node id, other_rev | on_rev << 1 | to_curr << 2) pairs
//!   packed decoding deltas to neighbor ids (0 = this node, odd = higher, even = lower)
//!   packed steps    (path, rev | start << 1 | end << 2 | del << 3,
//!                    prev, prev rank, next, next rank) where prev/next index decoding
//! path_count × path: u64 length, first step (u64 handle, u64 rank),
//!                    last step (u64 handle, u64 rank), u64 name length, name bytes
//! packed = u64 word count, words, u64 mask, u64 length, u8 width, u8 values per word
//! ```
//!
//! Edges are stored on both of their nodes (`to_curr` on the right one, self loops
//! once). A path is a linked list of steps; a step is named by its node handle
//! (`rank << 1 | is_reverse`) and its rank among the steps stored on that node.
//!
//! `docs/odgi_format.md` describes every field in detail, with a worked example
//! decoded from `odgi build` output; `utils/dump_odgi.py` prints a file's fields.
//!
//! Code map: `encoding` (little-endian integers and packed vectors, both
//! directions), `reader` (`records`: node records, `paths`: walking the step
//! lists) and `writer` (`numbering`: odgi ids, `edges` and `steps`: per-node
//! indexes of the records to write).

mod encoding;
mod reader;
mod writer;

#[cfg(test)]
mod tests;

use crate::core::core_types::NodeId;
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::PanResult;
use crate::traits::{GraphParser, GraphSerializer};
use std::io::{Read, Seek, Write};

/// Codec for odgi's binary graph format.
///
/// Node ids are written as odgi ids: numeric node names are kept when they are
/// distinct positive integers that fit a dense id range (as `odgi build` keeps
/// GFA segment ids), otherwise nodes are numbered `1..=n`. Parsed nodes are named
/// by their odgi id. Overlaps are not part of the format and are dropped.
pub struct ODGICodec;

impl<R: Read + Seek> GraphParser<R> for ODGICodec {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO> {
        reader::parse(reader)
    }
}

impl GraphSerializer for ODGICodec {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()> {
        writer::write(graph, writer)
    }
}

/// libhandlegraph magic number of odgi's `graph_t`.
pub(super) const ODGI_MAGIC: u32 = 1_988_148_666;

/// Values per edge record and per step record in a node.
pub(super) const EDGE_RECORD_LEN: usize = 2;
pub(super) const STEP_RECORD_LEN: usize = 6;

/// Bits of a packed edge.
pub(super) const EDGE_OTHER_REV: u64 = 1;
pub(super) const EDGE_ON_REV: u64 = 1 << 1;
pub(super) const EDGE_TO_CURR: u64 = 1 << 2;

/// Bits of a step type.
pub(super) const STEP_REV: u64 = 1;
pub(super) const STEP_START: u64 = 1 << 1;
pub(super) const STEP_END: u64 = 1 << 2;

/// Largest rank of a step on its node that is read or written (the
/// reader packs ranks into 31 bits).
pub(super) const MAX_STEP_RANK: u32 = u32::MAX >> 1;

/// Marks a deleted node rank.
pub(super) const VACANT: NodeId = NodeId::MAX;
