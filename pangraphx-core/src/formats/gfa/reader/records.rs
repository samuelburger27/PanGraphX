//! Pass 2: `L` and `P` records (and dispatch of `W` records to [`super::walks`]).

use super::RecordResult;
use super::blocks::{BlockReader, Split, lines, record_type};
use super::segments::{SegmentIndex, check_name};
use super::steps::parse_steps;
use super::walks::parse_walk;
use crate::core::core_types::{Edge, Orientation, Path};
use crate::error::PanResult;
use crate::formats::common::text::{line_error, lossy};
use log::{debug, warn};
use rayon::prelude::*;
use std::io::Read;

pub(super) fn read_links_and_paths<R: Read>(
    blocks: &mut BlockReader<'_, R>,
    index: &SegmentIndex<'_>,
    reference_samples: &[Vec<u8>],
    link_count: usize,
    path_count: usize,
) -> PanResult<(Vec<Edge>, Vec<Path>)> {
    let mut edges = Vec::with_capacity(link_count);
    let mut paths = Vec::with_capacity(path_count);
    let mut line_no = 0;

    while let Some(block) = blocks.next_block()? {
        let mut link_lines = Vec::new();
        let mut path_lines = Vec::new();
        for line in lines(block) {
            line_no += 1;
            match record_type(line) {
                b"L" => link_lines.push((line_no, line)),
                b"P" | b"W" => path_lines.push((line_no, line)),
                _ => {}
            }
        }

        let block_edges = link_lines
            .par_iter()
            .map(|&(n, line)| parse_link(line, index).map_err(|e| line_error("GFA", n, &e)))
            .collect::<PanResult<Vec<Edge>>>()?;
        edges.extend(block_edges);

        let block_paths = path_lines
            .par_iter()
            .map(|&(n, line)| {
                let path = if line.starts_with(b"W") {
                    parse_walk(line, index, reference_samples)
                } else {
                    parse_path(line, index)
                };
                path.map_err(|e| line_error("GFA", n, &e))
            })
            .collect::<PanResult<Vec<Path>>>()?;
        paths.extend(block_paths);
    }

    Ok((edges, paths))
}

pub(super) fn parse_orientation(field: &[u8]) -> RecordResult<Orientation> {
    match field {
        b"+" => Ok(Orientation::Forward),
        b"-" => Ok(Orientation::Reverse),
        _ => Err(format!("invalid orientation {:?}", lossy(field))),
    }
}

/// Converts a CIGAR string to an overlap length: the sum of its `M` operations.
/// `*` (unspecified) is 0. Other operations are ignored, as before.
pub(super) fn parse_overlap(cigar: &[u8]) -> RecordResult<u32> {
    if cigar == b"*" {
        return Ok(0);
    }
    let invalid = || format!("invalid CIGAR overlap {:?}", lossy(cigar));
    let mut total: u32 = 0;
    let mut count: Option<u32> = None;
    for &b in cigar {
        if b.is_ascii_digit() {
            let value = count
                .unwrap_or(0)
                .checked_mul(10)
                .and_then(|v| v.checked_add(u32::from(b - b'0')))
                .ok_or_else(invalid)?;
            count = Some(value);
        } else if b"MIDNSHPX=".contains(&b) {
            let length = count.take().ok_or_else(invalid)?;
            if b == b'M' {
                total = total.checked_add(length).ok_or_else(invalid)?;
            } else {
                debug!(
                    "Ignoring non-match CIGAR op {} with count {length}",
                    b as char
                );
            }
        } else {
            return Err(invalid());
        }
    }
    if cigar.is_empty() || count.is_some() {
        return Err(invalid());
    }
    Ok(total)
}

/// Parses `L <from> <orient> <to> <orient> <overlap> [tags]`.
pub(super) fn parse_link(line: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Edge> {
    let mut fields = Split::new(line, b'\t').skip(1);
    let mut field = |what: &str| {
        fields
            .next()
            .ok_or_else(|| format!("L line without {what}"))
    };
    let from = field("a from segment")?;
    let from_orient = field("a from orientation")?;
    let to = field("a to segment")?;
    let to_orient = field("a to orientation")?;
    let overlap = field("an overlap")?;
    Ok(Edge {
        from_node: index.resolve_or_err(from)?,
        from_orient: parse_orientation(from_orient)?,
        to_node: index.resolve_or_err(to)?,
        to_orient: parse_orientation(to_orient)?,
        overlap: parse_overlap(overlap)?,
    })
}

/// Parses `P <name> <steps> <overlaps> [tags]`.
pub(super) fn parse_path(line: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Path> {
    let mut fields = Split::new(line, b'\t').skip(1);
    let name = fields.next().ok_or("P line without a path name")?;
    check_name(name, "path name")?;
    let steps_field = fields
        .next()
        .ok_or_else(|| format!("path {} has no segment list", lossy(name)))?;
    let overlaps_field = fields
        .next()
        .ok_or_else(|| format!("path {} has no overlap field", lossy(name)))?;

    let steps =
        parse_steps(steps_field, index).map_err(|e| format!("path {}: {e}", lossy(name)))?;

    let overlaps = if overlaps_field == b"*" {
        Vec::new()
    } else {
        Split::new(overlaps_field, b',')
            .map(parse_overlap)
            .collect::<RecordResult<Vec<u32>>>()
            .map_err(|e| format!("path {}: {e}", lossy(name)))?
    };
    if !overlaps.is_empty() && overlaps.len() + 1 != steps.len() {
        warn!(
            "GFA path {} has {} overlaps for {} steps (expected {})",
            lossy(name),
            overlaps.len(),
            steps.len(),
            steps.len().saturating_sub(1)
        );
    }

    Ok(Path {
        name: name.to_vec(),
        steps,
        overlaps,
    })
}
