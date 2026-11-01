//! `W` records (GFA 1.1 walks) and the vg-compatible names of their paths.

use super::RecordResult;
use super::blocks::Split;
use super::segments::{SegmentIndex, check_name};
use super::steps::parse_walk_steps;
use crate::core::core_types::Path;
use crate::formats::common::text::lossy;

/// Parses `W <sample> <haplotype> <sequence id> <start> <end> <walk> [tags]`.
pub(super) fn parse_walk(
    line: &[u8],
    index: &SegmentIndex<'_>,
    reference_samples: &[Vec<u8>],
) -> RecordResult<Path> {
    let mut fields = Split::new(line, b'\t').skip(1);
    let mut field = |what: &str| {
        fields
            .next()
            .ok_or_else(|| format!("W line without {what}"))
    };
    let sample = field("a sample name")?;
    let haplotype = field("a haplotype index")?;
    let seq_id = field("a sequence id")?;
    let start = field("a start position")?;
    let end = field("an end position")?;
    let walk = field("a walk")?;

    check_name(sample, "sample name")?;
    check_name(seq_id, "sequence id")?;
    let name = walk_path_name(
        sample,
        parse_natural(haplotype, "haplotype index")?,
        seq_id,
        parse_position(start, "start position")?,
        parse_position(end, "end position")?,
        reference_samples,
    );
    let steps = parse_walk_steps(walk, index).map_err(|e| format!("walk {}: {e}", lossy(&name)))?;
    Ok(Path {
        name,
        steps,
        overlaps: Vec::new(),
    })
}

/// Parses a non-negative decimal integer.
pub(super) fn parse_natural(field: &[u8], what: &str) -> RecordResult<u64> {
    let invalid = || format!("invalid {what} {:?}", lossy(field));
    if field.is_empty() {
        return Err(invalid());
    }
    field
        .iter()
        .try_fold(0u64, |value, &b| {
            if !b.is_ascii_digit() {
                return None;
            }
            value.checked_mul(10)?.checked_add(u64::from(b - b'0'))
        })
        .ok_or_else(invalid)
}

/// Parses a walk start or end: an integer, or `*` if unknown.
pub(super) fn parse_position(field: &[u8], what: &str) -> RecordResult<Option<u64>> {
    if field == b"*" {
        Ok(None)
    } else {
        parse_natural(field, what).map(Some)
    }
}

/// Names a walk the way vg's GFA reader does (libhandlegraph
/// `PathMetadata::create_path_name`; checked against vg 1.73):
///
/// - sample `*`: generic path `seq_id`
/// - sample listed in an `RS` header tag: reference path `sample#haplotype#seq_id`
/// - any other sample: haplotype path `sample#haplotype#seq_id#0`
///
/// followed by the subrange `[start-end]` (`[start]` if the end is `*`), unless
/// the start is 0 or `*`. The GBZ reader (`gbz::reader::path_name`) builds the
/// same names from GBZ metadata, which keeps only the subrange start.
pub(super) fn walk_path_name(
    sample: &[u8],
    haplotype: u64,
    seq_id: &[u8],
    start: Option<u64>,
    end: Option<u64>,
    reference_samples: &[Vec<u8>],
) -> Vec<u8> {
    let mut itoa_buf = itoa::Buffer::new();
    let mut name = Vec::with_capacity(sample.len() + seq_id.len() + 48);
    let generic = sample == b"*";
    if !generic {
        name.extend_from_slice(sample);
        name.push(b'#');
        name.extend_from_slice(itoa_buf.format(haplotype).as_bytes());
        name.push(b'#');
    }
    name.extend_from_slice(seq_id);
    if !generic && !reference_samples.iter().any(|s| s == sample) {
        name.extend_from_slice(b"#0");
    }
    if let Some(start) = start.filter(|&start| start != 0) {
        name.push(b'[');
        name.extend_from_slice(itoa_buf.format(start).as_bytes());
        if let Some(end) = end {
            name.push(b'-');
            name.extend_from_slice(itoa_buf.format(end).as_bytes());
        }
        name.push(b']');
    }
    name
}
