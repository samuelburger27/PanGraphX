//! Streaming GFA 1.0 / 1.1 parser.
//!
//! The input is read twice, in bounded blocks of whole lines, so the file text
//! is never fully resident:
//!
//! 1. **Segments.** `S` lines become nodes (in file order) and their names are
//!    indexed. `L`/`P`/`W` lines are only counted, to reserve exact capacities,
//!    and the `RS` tag of `H` lines lists the reference samples.
//! 2. **Links and paths.** With the name index complete and read-only, `L`,
//!    `P` and `W` lines are parsed in parallel. This also makes the parser
//!    independent of record order: a link or path may precede the segments it
//!    uses.
//!
//! `W` lines (GFA 1.1 walks) become paths in file order among the `P` lines,
//! named from their sample, haplotype, sequence id and range the way vg names
//! them (see [`walk_path_name`](walks::walk_path_name)). Records of other
//! types (`C`, `J`, comments, ...) are skipped. Malformed `S`, `L`, `P` and `W`
//! records are reported as [`PanGraphXError::Parse`](crate::error::PanGraphXError::Parse)
//! with their line number.
//!
//! Submodules: [`blocks`] (input splitting), [`segments`] (pass 1 and the name
//! index), [`records`] (pass 2), [`walks`] and [`steps`] (`W` lines and step lists).

mod blocks;
mod records;
mod segments;
mod steps;
mod walks;

#[cfg(test)]
mod tests;

use crate::core::graph_dto::CoreGraphDTO;
use crate::error::PanResult;
use crate::formats::common::text::{line_error, lossy};
use blocks::BlockReader;
use records::read_links_and_paths;
use segments::{SegmentIndex, read_segments, segment_line};
use std::io::{Read, Seek, SeekFrom};

/// Target size of a block of lines read at once.
const BLOCK_SIZE: usize = 16 << 20;

/// Result of parsing a single record; the message is prefixed with the line number by the caller.
type RecordResult<T> = Result<T, String>;

/// Parses a GFA stream into a [`CoreGraphDTO`].
///
/// The reader is read from its current position to the end, twice.
pub fn parse<R: Read + Seek>(reader: &mut R) -> PanResult<CoreGraphDTO> {
    parse_with_block_size(reader, BLOCK_SIZE)
}

fn parse_with_block_size<R: Read + Seek>(
    reader: &mut R,
    block_size: usize,
) -> PanResult<CoreGraphDTO> {
    let start = reader.stream_position()?;
    let segments = read_segments(&mut BlockReader::new(reader, block_size))?;

    let index = match SegmentIndex::new(&segments.names) {
        Ok(index) => index,
        Err(duplicate) => {
            // Duplicates are rare, so their line number is found by reading again
            // rather than storing the line of every segment.
            reader.seek(SeekFrom::Start(start))?;
            let line_no = segment_line(&mut BlockReader::new(reader, block_size), duplicate)?;
            return Err(line_error(
                "GFA",
                line_no,
                &format!(
                    "duplicate segment name {}",
                    lossy(&segments.names[duplicate])
                ),
            ));
        }
    };
    reader.seek(SeekFrom::Start(start))?;
    let (edges, paths) = read_links_and_paths(
        &mut BlockReader::new(reader, block_size),
        &index,
        &segments.reference_samples,
        segments.link_count,
        segments.path_count,
    )?;
    drop(index);

    let node_name_map = segments.names.into_iter().enumerate().collect();
    Ok(CoreGraphDTO {
        nodes: segments.nodes,
        edges,
        paths,
        node_name_map: Some(node_name_map),
    })
}
