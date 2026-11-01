//! Step lists of `P` lines and walks of `W` lines, parsed in parallel pieces when long.

use super::RecordResult;
use super::blocks::Split;
use super::segments::SegmentIndex;
use crate::core::core_types::{Orientation, Step};
use crate::formats::common::text::lossy;
use memchr::{memchr, memchr_iter, memchr2};
use rayon::prelude::*;

/// The step list of a long `P` or `W` line is split into pieces of about this many
/// bytes, which are parsed in parallel.
pub(super) const PATH_PIECE_SIZE: usize = 1 << 20;

/// Splits a comma-separated list into pieces of about `piece_size` bytes,
/// cutting only at commas (which are dropped).
pub(super) fn split_at_commas(list: &[u8], piece_size: usize) -> Vec<&[u8]> {
    let mut pieces = Vec::with_capacity(list.len() / piece_size + 1);
    let mut rest = list;
    while rest.len() > piece_size {
        let Some(offset) = memchr(b',', &rest[piece_size..]) else {
            break;
        };
        let cut = piece_size + offset;
        pieces.push(&rest[..cut]);
        rest = &rest[cut + 1..];
    }
    pieces.push(rest);
    pieces
}

/// Splits a walk `>name<name...` into pieces of about `piece_size` bytes,
/// cutting only before an orientation (which stays with its step).
pub(super) fn split_at_orientations(walk: &[u8], piece_size: usize) -> Vec<&[u8]> {
    let mut pieces = Vec::with_capacity(walk.len() / piece_size + 1);
    let mut rest = walk;
    while rest.len() > piece_size {
        let Some(offset) = memchr2(b'>', b'<', &rest[piece_size..]) else {
            break;
        };
        let cut = piece_size + offset;
        pieces.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    pieces.push(rest);
    pieces
}

/// Parses a step list `name+,name-,...`; long lists are parsed in parallel pieces.
pub(super) fn parse_steps(list: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Vec<Step>> {
    parse_pieces(&split_at_commas(list, PATH_PIECE_SIZE), |piece| {
        parse_step_piece(piece, index)
    })
}

/// Parses a walk `>name<name...`; long walks are parsed in parallel pieces.
pub(super) fn parse_walk_steps(walk: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Vec<Step>> {
    if walk.is_empty() {
        return Err("empty walk".to_string());
    }
    parse_pieces(&split_at_orientations(walk, PATH_PIECE_SIZE), |piece| {
        parse_walk_piece(piece, index)
    })
}

/// Parses the pieces of a step list, in parallel if there are several, and
/// concatenates their steps.
pub(super) fn parse_pieces<F>(pieces: &[&[u8]], parse_piece: F) -> RecordResult<Vec<Step>>
where
    F: Fn(&[u8]) -> RecordResult<Vec<Step>> + Sync,
{
    if let [piece] = pieces {
        return parse_piece(piece);
    }
    let parsed = pieces
        .par_iter()
        .map(|piece| parse_piece(piece))
        .collect::<RecordResult<Vec<Vec<Step>>>>()?;
    let mut steps = Vec::with_capacity(parsed.iter().map(Vec::len).sum());
    for piece in parsed {
        steps.extend(piece);
    }
    Ok(steps)
}

pub(super) fn parse_step_piece(piece: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Vec<Step>> {
    let mut steps = Vec::with_capacity(memchr_iter(b',', piece).count() + 1);
    for item in Split::new(piece, b',') {
        let Some((&orientation, name)) = item.split_last() else {
            return Err("empty step".to_string());
        };
        let orientation = match orientation {
            b'+' => Orientation::Forward,
            b'-' => Orientation::Reverse,
            _ => return Err(format!("step {:?} has no orientation", lossy(item))),
        };
        if name.is_empty() {
            return Err(format!("invalid step {:?}", lossy(item)));
        }
        let node_id = index.resolve_or_err(name)?;
        steps.push(Step::new(node_id, orientation).map_err(|e| e.to_string())?);
    }
    Ok(steps)
}

pub(super) fn parse_walk_piece(piece: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Vec<Step>> {
    // A plain count vectorizes; `memchr2_iter(..).count()` visits every match.
    let step_count = piece.iter().filter(|&&b| b == b'>' || b == b'<').count();
    let mut steps = Vec::with_capacity(step_count);
    let mut rest = piece;
    while let Some((&orientation, after)) = rest.split_first() {
        let orientation = match orientation {
            b'>' => Orientation::Forward,
            b'<' => Orientation::Reverse,
            // Pieces are cut before orientations, so only a walk can start this way.
            _ => return Err("walk does not start with > or <".to_string()),
        };
        let end = memchr2(b'>', b'<', after).unwrap_or(after.len());
        let name = &after[..end];
        if name.is_empty() {
            return Err("empty walk step".to_string());
        }
        let node_id = index.resolve_or_err(name)?;
        steps.push(Step::new(node_id, orientation).map_err(|e| e.to_string())?);
        rest = &after[end..];
    }
    Ok(steps)
}
