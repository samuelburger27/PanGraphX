//! ODGI reader: the header, the node records (decoded in parallel blocks, see
//! [`records`]) and the path metadata, after which every path is walked
//! through the stored steps (see [`paths`]).

mod paths;
mod records;

#[cfg(test)]
pub(super) use records::{BLOCK_BYTES, scan_record};

use crate::core::core_types::{NodeId, Nodes};
use crate::core::graph_dto::{CoreGraphDTO, GraphParts};
use crate::error::{PanGraphXError, PanResult};
use crate::formats::odgi::encoding::{MAX_PREALLOC, read_bytes, read_u64, to_usize, truncated};
use crate::formats::odgi::{ODGI_MAGIC, VACANT};
use log::warn;
use paths::{PathMeta, StepLink, StepTable};
use records::{MIN_RECORD_BYTES, NodeSection, RecordContext, next_block};
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, SeekFrom};

/// Reads an ODGI file, with or without its leading magic number.
pub(super) fn parse<R: Read + Seek>(reader: &mut R) -> PanResult<CoreGraphDTO> {
    let mut reader = BufReader::with_capacity(1 << 20, reader);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).map_err(truncated)?;
    if u32::from_be_bytes(magic) != ODGI_MAGIC {
        // odgi accepts files without the magic number (older versions), so do we.
        warn!("ODGI magic number not found; reading the file anyway");
        reader.seek_relative(-4)?;
    }
    let position = reader.stream_position()?;
    let input_len = reader.seek(SeekFrom::End(0))?.saturating_sub(position);
    reader.seek(SeekFrom::Start(position))?;
    read_graph(&mut reader, input_len)
}

/// Builds the node name map. Inserting is a chain of cache misses, so it is
/// done block by block alongside the decoding of the next block.
struct NodeNamer {
    names: HashMap<NodeId, Vec<u8>>,
    /// odgi id of the last rank added.
    last_id: u64,
}

impl NodeNamer {
    /// Names the nodes of the next ranks, given their `rank_to_node` entries.
    fn add(&mut self, ranks: &[NodeId]) {
        let mut id_buf = itoa::Buffer::new();
        for &node in ranks {
            // Checked in `read_graph` not to overflow.
            self.last_id += 1;
            if node != VACANT {
                self.names
                    .insert(node, id_buf.format(self.last_id).as_bytes().to_vec());
            }
        }
    }
}

/// Reads the node records in blocks: record boundaries are found sequentially
/// from their length fields, then the records of a block are decoded in
/// parallel while `namer` names the nodes of the previous block. Returns the
/// records, the input left after them and the ranks still to be named.
fn read_node_section<R: Read>(
    reader: &mut R,
    context: RecordContext,
    mut input_left: u64,
    namer: &mut NodeNamer,
) -> PanResult<(NodeSection, Vec<u8>, Vec<NodeId>)> {
    let count = context.record_count;
    // `record_count` is bounded by the input size, so these reservations are too.
    let mut section = NodeSection {
        nodes: Nodes::new(),
        edges: Vec::new(),
        offsets: Vec::with_capacity(count + 1),
        links: Vec::new(),
        rank_to_node: Vec::with_capacity(count),
    };
    section.offsets.push(0);
    let mut buf: Vec<u8> = Vec::new();
    let mut unnamed = Vec::new();
    let mut rank = 0;
    while rank < count {
        let spans = next_block(reader, &mut buf, &mut input_left, count - rank)?;
        let (decoded, ()) = rayon::join(
            || section.decode_block(&buf, &spans, rank, context),
            || namer.add(&unnamed),
        );
        decoded?;
        unnamed = section.rank_to_node[rank..].to_vec();
        rank += spans.len();
        let consumed = spans.last().map_or(0, |span| span.end);
        buf.drain(..consumed);
    }
    section.resolve_edges()?;
    Ok((section, buf, unnamed))
}

/// Reads the graph after the magic number; `input_len` is the number of bytes
/// left in the input, which bounds the node count and the reads.
fn read_graph<R: Read>(reader: &mut R, input_len: u64) -> PanResult<CoreGraphDTO> {
    let _max_node_id = read_u64(reader)?;
    let _min_node_id = read_u64(reader)?;
    let record_count = to_usize(read_u64(reader)?, "node count")?;
    let _edge_count = read_u64(reader)?;
    let path_count = to_usize(read_u64(reader)?, "path count")?;
    let _path_handle_next = read_u64(reader)?;
    let id_increment = read_u64(reader)?;
    let input_left = input_len.saturating_sub(7 * 8);
    // Node ranks must fit in `u32` below `StepLink::END`.
    if record_count >= StepLink::END as usize || record_count as u64 > input_left / MIN_RECORD_BYTES
    {
        return Err(PanGraphXError::Parse(format!(
            "ODGI node count {record_count} is too large"
        )));
    }
    if id_increment.checked_add(record_count as u64 + 1).is_none() {
        return Err(PanGraphXError::Parse(format!(
            "ODGI id increment {id_increment} is too large"
        )));
    }
    let context = RecordContext {
        record_count,
        id_increment,
    };
    let mut namer = NodeNamer {
        // `record_count` is bounded by the input size.
        names: HashMap::with_capacity(record_count),
        last_id: id_increment,
    };
    let (section, rest, unnamed) = read_node_section(reader, context, input_left, &mut namer)?;

    let mut reader = rest.as_slice().chain(reader);
    let mut metas = Vec::with_capacity(path_count.min(MAX_PREALLOC));
    for _ in 0..path_count {
        let length = read_u64(&mut reader)?;
        let first_handle = read_u64(&mut reader)?;
        let first_rank = read_u64(&mut reader)?;
        let _last_handle = read_u64(&mut reader)?;
        let _last_rank = read_u64(&mut reader)?;
        let name_len = read_u64(&mut reader)?;
        let name = read_bytes(&mut reader, name_len)?;
        metas.push(PathMeta {
            name,
            length,
            first_handle,
            first_rank,
        });
    }

    let NodeSection {
        nodes,
        edges,
        offsets,
        links,
        rank_to_node,
    } = section;
    let table = StepTable {
        offsets,
        links,
        rank_to_node,
    };
    let ((), paths) = rayon::join(|| namer.add(&unnamed), || table.walk_all(metas));
    CoreGraphDTO::try_from(GraphParts {
        nodes,
        edges,
        paths: paths?,
        node_name_map: Some(namer.names),
    })
}
