//! Pass 1: `S` records, record counts and reference samples; the segment name index.

use super::RecordResult;
use super::blocks::{BlockReader, Split, lines, note_skipped, record_type};
use crate::core::core_types::{NodeId, NodeName, Nodes, Step};
use crate::error::{PanGraphXError, PanResult};
use crate::formats::common::id_map::{IdMap, parse_canonical_int};
use crate::formats::common::text::{line_error, lossy};
use rayon::prelude::*;
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::io::Read;

/// Everything pass 1 collects.
pub(super) struct Segments {
    pub(super) nodes: Nodes,
    /// Segment names, indexed by node id.
    pub(super) names: Vec<NodeName>,
    pub(super) link_count: usize,
    /// Number of `P` and `W` lines.
    pub(super) path_count: usize,
    /// Samples listed in `RS` header tags.
    pub(super) reference_samples: Vec<Vec<u8>>,
}

pub(super) fn read_segments<R: Read>(blocks: &mut BlockReader<'_, R>) -> PanResult<Segments> {
    let mut segments = Segments {
        nodes: Nodes::new(),
        names: Vec::new(),
        link_count: 0,
        path_count: 0,
        reference_samples: Vec::new(),
    };
    let mut skipped = Vec::new();
    let mut line_no = 0;

    while let Some(block) = blocks.next_block()? {
        let mut segment_lines = Vec::new();
        for line in lines(block) {
            line_no += 1;
            match record_type(line) {
                b"S" => segment_lines.push((line_no, line)),
                b"L" => segments.link_count += 1,
                b"P" | b"W" => segments.path_count += 1,
                b"H" => add_reference_samples(line, &mut segments.reference_samples),
                b"" => {}
                other => note_skipped(&mut skipped, other),
            }
        }

        let parsed = segment_lines
            .par_iter()
            .map(|&(n, line)| parse_segment(line).map_err(|e| line_error("GFA", n, &e)))
            .collect::<PanResult<Vec<_>>>()?;
        for (name, sequence) in parsed {
            segments.nodes.push(sequence.to_vec());
            segments.names.push(name.to_vec());
        }
    }

    if segments.nodes.len() > Step::MAX_NODE_ID + 1 {
        return Err(PanGraphXError::Parse(format!(
            "GFA has {} segments, more than the supported maximum of {}",
            segments.nodes.len(),
            Step::MAX_NODE_ID + 1
        )));
    }
    Ok(segments)
}

/// Adds the samples of an `RS:Z:` header tag, a space-separated list that
/// marks the walks of those samples as reference paths.
pub(super) fn add_reference_samples(header: &[u8], samples: &mut Vec<Vec<u8>>) {
    for tag in Split::new(header, b'\t').skip(1) {
        if let Some(list) = tag.strip_prefix(b"RS:Z:") {
            samples.extend(
                Split::new(list, b' ')
                    .filter(|sample| !sample.is_empty())
                    .map(<[u8]>::to_vec),
            );
        }
    }
}

/// Returns the line number of the `S` record that defines node `id`.
pub(super) fn segment_line<R: Read>(
    blocks: &mut BlockReader<'_, R>,
    id: NodeId,
) -> PanResult<usize> {
    let mut line_no = 0;
    let mut segment = 0;
    while let Some(block) = blocks.next_block()? {
        for line in lines(block) {
            line_no += 1;
            if record_type(line) == b"S" {
                if segment == id {
                    return Ok(line_no);
                }
                segment += 1;
            }
        }
    }
    Err(PanGraphXError::Parse(format!(
        "GFA changed while reading: segment {id} not found"
    )))
}

/// Bytes allowed in a segment sequence (GFA 1.0: `\*|[A-Za-z=.]+`).
pub(super) const fn is_sequence_byte(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'=' || b == b'.'
}

/// Checks that a segment or path name is non-empty printable ASCII without spaces.
pub(super) fn check_name(name: &[u8], what: &str) -> RecordResult<()> {
    if name.is_empty() {
        return Err(format!("empty {what}"));
    }
    if !name.iter().all(u8::is_ascii_graphic) {
        return Err(format!("invalid {what} {:?}", lossy(name)));
    }
    Ok(())
}

/// Parses `S <name> <sequence> [tags]` into its name and sequence.
pub(super) fn parse_segment(line: &[u8]) -> RecordResult<(&[u8], &[u8])> {
    let mut fields = Split::new(line, b'\t').skip(1);
    let name = fields.next().ok_or("S line without a segment name")?;
    check_name(name, "segment name")?;
    let sequence = fields
        .next()
        .ok_or_else(|| format!("segment {} has no sequence field", lossy(name)))?;
    if sequence != b"*" && (sequence.is_empty() || !sequence.iter().copied().all(is_sequence_byte))
    {
        return Err(format!("segment {} has an invalid sequence", lossy(name)));
    }
    Ok((name, sequence))
}

/// Resolves segment names to node ids.
pub(super) enum SegmentIndex<'a> {
    /// All names are canonical integers: a lookup table, no hashing.
    /// References use the same canonical form, so `007` never matches `7`.
    Numeric(IdMap),
    Named(FxHashMap<&'a [u8], NodeId>),
}

impl<'a> SegmentIndex<'a> {
    /// Indexes `names` (by node id).
    ///
    /// # Errors
    /// Returns the node id of the second segment with a name already used.
    pub(super) fn new(names: &'a [NodeName]) -> Result<Self, NodeId> {
        let numeric: Option<Vec<i64>> = names.iter().map(|n| parse_canonical_int(n)).collect();
        if let Some(ids) = numeric {
            return IdMap::from_ids(&ids).map(Self::Numeric).map_err(|dup| {
                // `from_ids` stops at the second occurrence of the duplicate.
                ids.iter()
                    .enumerate()
                    .filter(|&(_, &id)| id == dup)
                    .nth(1)
                    .map_or(0, |(i, _)| i)
            });
        }

        let mut map = FxHashMap::with_capacity_and_hasher(names.len(), FxBuildHasher);
        for (id, name) in names.iter().enumerate() {
            if map.insert(name.as_slice(), id).is_some() {
                return Err(id);
            }
        }
        Ok(Self::Named(map))
    }

    fn resolve(&self, name: &[u8]) -> Option<NodeId> {
        match self {
            Self::Numeric(map) => map.get(parse_canonical_int(name)?),
            Self::Named(map) => map.get(name).copied(),
        }
    }

    pub(super) fn resolve_or_err(&self, name: &[u8]) -> RecordResult<NodeId> {
        self.resolve(name)
            .ok_or_else(|| format!("reference to undefined segment {}", lossy(name)))
    }
}
