//! Streaming GFA 1.0 parser.
//!
//! The input is read twice, in bounded blocks of whole lines, so the file text
//! is never fully resident:
//!
//! 1. **Segments.** `S` lines become nodes (in file order) and their names are
//!    indexed. `L`/`P` lines are only counted, to reserve exact capacities.
//! 2. **Links and paths.** With the name index complete and read-only, `L` and
//!    `P` lines are parsed in parallel. This also makes the parser independent
//!    of record order: a link or path may precede the segments it uses.
//!
//! Records of other types (`H`, `C`, `W`, `J`, comments, ...) are skipped.
//! Malformed `S`, `L` and `P` records are reported as
//! [`PanGraphXError::Parse`] with their line number.

use crate::core::core_types::{Edge, NodeId, NodeName, Nodes, Orientation, Path, Step};
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::id_map::IdMap;
use log::{debug, warn};
use memchr::{memchr, memchr_iter, memrchr};
use rayon::prelude::*;
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::io::{ErrorKind, Read, Seek, SeekFrom};

/// Target size of a block of lines read at once.
const BLOCK_SIZE: usize = 16 << 20;

/// The step list of a long `P` line is split into pieces of about this many
/// bytes, which are parsed in parallel.
const PATH_PIECE_SIZE: usize = 1 << 20;

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

fn line_error(line_no: usize, message: &str) -> PanGraphXError {
    PanGraphXError::Parse(format!("GFA line {line_no}: {message}"))
}

fn lossy(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

// ---------------------------------------------------------------------------
// Input splitting
// ---------------------------------------------------------------------------

/// Reads the input in blocks that end on a line boundary.
///
/// A block holds at least `block_size` bytes unless the input ends first; a
/// line longer than that grows the block until the line is complete.
struct BlockReader<'r, R> {
    reader: &'r mut R,
    buf: Vec<u8>,
    /// Length of the block returned last; the rest of `buf` is an incomplete line.
    consumed: usize,
    block_size: usize,
    eof: bool,
}

impl<'r, R: Read> BlockReader<'r, R> {
    fn new(reader: &'r mut R, block_size: usize) -> Self {
        Self {
            reader,
            buf: Vec::new(),
            consumed: 0,
            block_size: block_size.max(1),
            eof: false,
        }
    }

    /// Returns the next block of whole lines, or `None` at the end of the input.
    /// The last line of the input may lack its newline.
    fn next_block(&mut self) -> PanResult<Option<&[u8]>> {
        self.buf.drain(..self.consumed);
        self.consumed = 0;
        let mut target = self.block_size;
        loop {
            self.fill_to(target)?;
            if let Some(last_newline) = memrchr(b'\n', &self.buf) {
                self.consumed = last_newline + 1;
                return Ok(Some(&self.buf[..self.consumed]));
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                self.consumed = self.buf.len();
                return Ok(Some(&self.buf));
            }
            // A single line is longer than the block: keep reading.
            target = self.buf.len().saturating_mul(2);
        }
    }

    fn fill_to(&mut self, target: usize) -> PanResult<()> {
        while !self.eof && self.buf.len() < target {
            let len = self.buf.len();
            self.buf.resize(target, 0);
            let read = self.reader.read(&mut self.buf[len..]);
            match read {
                Ok(0) => {
                    self.buf.truncate(len);
                    self.eof = true;
                }
                Ok(n) => self.buf.truncate(len + n),
                Err(e) if e.kind() == ErrorKind::Interrupted => self.buf.truncate(len),
                Err(e) => {
                    self.buf.truncate(len);
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }
}

/// Iterator over the parts of a byte string separated by `sep`.
///
/// An empty input yields one empty part, like [`slice::split`], but the
/// separator search uses `memchr`.
struct Split<'a> {
    rest: Option<&'a [u8]>,
    sep: u8,
}

impl<'a> Split<'a> {
    const fn new(bytes: &'a [u8], sep: u8) -> Self {
        Self {
            rest: Some(bytes),
            sep,
        }
    }
}

impl<'a> Iterator for Split<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        let rest = self.rest?;
        if let Some(i) = memchr(self.sep, rest) {
            self.rest = Some(&rest[i + 1..]);
            Some(&rest[..i])
        } else {
            self.rest = None;
            Some(rest)
        }
    }
}

/// Iterator over the lines of a block, without `\n` / `\r\n` terminators.
struct Lines<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        let (line, rest) = memchr(b'\n', self.rest).map_or((self.rest, &[][..]), |i| {
            (&self.rest[..i], &self.rest[i + 1..])
        });
        self.rest = rest;
        Some(line.strip_suffix(b"\r").unwrap_or(line))
    }
}

const fn lines(block: &[u8]) -> Lines<'_> {
    Lines { rest: block }
}

/// The record type of a line: its first tab-separated field (`#` for comments).
fn record_type(line: &[u8]) -> &[u8] {
    if line.first() == Some(&b'#') {
        return b"#";
    }
    &line[..memchr(b'\t', line).unwrap_or(line.len())]
}

/// Logs each skipped record type once.
fn note_skipped(seen: &mut Vec<Vec<u8>>, record_type: &[u8]) {
    if record_type != b"H" && !seen.iter().any(|t| t == record_type) {
        debug!("Skipping GFA records of type {:?}", lossy(record_type));
        seen.push(record_type.to_vec());
    }
}

// ---------------------------------------------------------------------------
// Pass 1: segments
// ---------------------------------------------------------------------------

/// Everything pass 1 collects.
struct Segments {
    nodes: Nodes,
    /// Segment names, indexed by node id.
    names: Vec<NodeName>,
    link_count: usize,
    path_count: usize,
}

fn read_segments<R: Read>(blocks: &mut BlockReader<'_, R>) -> PanResult<Segments> {
    let mut segments = Segments {
        nodes: Nodes::new(),
        names: Vec::new(),
        link_count: 0,
        path_count: 0,
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
                b"P" => segments.path_count += 1,
                b"" => {}
                other => note_skipped(&mut skipped, other),
            }
        }

        let parsed = segment_lines
            .par_iter()
            .map(|&(n, line)| parse_segment(line).map_err(|e| line_error(n, &e)))
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

/// Returns the line number of the `S` record that defines node `id`.
fn segment_line<R: Read>(blocks: &mut BlockReader<'_, R>, id: NodeId) -> PanResult<usize> {
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
const fn is_sequence_byte(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'=' || b == b'.'
}

/// Checks that a segment or path name is non-empty printable ASCII without spaces.
fn check_name(name: &[u8], what: &str) -> RecordResult<()> {
    if name.is_empty() {
        return Err(format!("empty {what}"));
    }
    if !name.iter().all(u8::is_ascii_graphic) {
        return Err(format!("invalid {what} {:?}", lossy(name)));
    }
    Ok(())
}

/// Parses `S <name> <sequence> [tags]` into its name and sequence.
fn parse_segment(line: &[u8]) -> RecordResult<(&[u8], &[u8])> {
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

// ---------------------------------------------------------------------------
// Segment name lookup
// ---------------------------------------------------------------------------

/// Parses a canonical non-negative decimal integer: digits only, no leading
/// zeros (except `0` itself), at most 18 digits so it always fits an `i64`.
fn parse_canonical_int(name: &[u8]) -> Option<i64> {
    if name.is_empty() || name.len() > 18 || (name.len() > 1 && name[0] == b'0') {
        return None;
    }
    name.iter().try_fold(0_i64, |value, &b| {
        b.is_ascii_digit().then(|| value * 10 + i64::from(b - b'0'))
    })
}

/// Resolves segment names to node ids.
enum SegmentIndex<'a> {
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
    fn new(names: &'a [NodeName]) -> Result<Self, NodeId> {
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

    fn resolve_or_err(&self, name: &[u8]) -> RecordResult<NodeId> {
        self.resolve(name)
            .ok_or_else(|| format!("reference to undefined segment {}", lossy(name)))
    }
}

// ---------------------------------------------------------------------------
// Pass 2: links and paths
// ---------------------------------------------------------------------------

fn read_links_and_paths<R: Read>(
    blocks: &mut BlockReader<'_, R>,
    index: &SegmentIndex<'_>,
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
                b"P" => path_lines.push((line_no, line)),
                _ => {}
            }
        }

        let block_edges = link_lines
            .par_iter()
            .map(|&(n, line)| parse_link(line, index).map_err(|e| line_error(n, &e)))
            .collect::<PanResult<Vec<Edge>>>()?;
        edges.extend(block_edges);

        let block_paths = path_lines
            .par_iter()
            .map(|&(n, line)| parse_path(line, index).map_err(|e| line_error(n, &e)))
            .collect::<PanResult<Vec<Path>>>()?;
        paths.extend(block_paths);
    }

    Ok((edges, paths))
}

fn parse_orientation(field: &[u8]) -> RecordResult<Orientation> {
    match field {
        b"+" => Ok(Orientation::Forward),
        b"-" => Ok(Orientation::Reverse),
        _ => Err(format!("invalid orientation {:?}", lossy(field))),
    }
}

/// Converts a CIGAR string to an overlap length: the sum of its `M` operations.
/// `*` (unspecified) is 0. Other operations are ignored, as before.
fn parse_overlap(cigar: &[u8]) -> RecordResult<u32> {
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
fn parse_link(line: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Edge> {
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
fn parse_path(line: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Path> {
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

/// Splits a comma-separated list into pieces of about `piece_size` bytes,
/// cutting only at commas (which are dropped).
fn split_at_commas(list: &[u8], piece_size: usize) -> Vec<&[u8]> {
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

/// Parses a step list `name+,name-,...`; long lists are parsed in parallel pieces.
fn parse_steps(list: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Vec<Step>> {
    let pieces = split_at_commas(list, PATH_PIECE_SIZE);
    if let [piece] = pieces.as_slice() {
        return parse_step_piece(piece, index);
    }
    let parsed = pieces
        .par_iter()
        .map(|piece| parse_step_piece(piece, index))
        .collect::<RecordResult<Vec<Vec<Step>>>>()?;
    let mut steps = Vec::with_capacity(parsed.iter().map(Vec::len).sum());
    for piece in parsed {
        steps.extend(piece);
    }
    Ok(steps)
}

fn parse_step_piece(piece: &[u8], index: &SegmentIndex<'_>) -> RecordResult<Vec<Step>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::io::Cursor;

    fn parse_str(text: &str) -> PanResult<CoreGraphDTO> {
        parse(&mut Cursor::new(text.as_bytes()))
    }

    fn err(text: &str) -> String {
        parse_str(text).unwrap_err().to_string()
    }

    fn step_names(graph: &CoreGraphDTO, path: usize) -> Vec<(String, Orientation)> {
        graph.paths[path]
            .steps
            .iter()
            .map(|s| (graph.get_name_from_id(s.node_id()), s.orientation()))
            .collect()
    }

    const SMALL: &str = "H\tVN:Z:1.0\n\
        S\t1\tACGT\n\
        S\t2\tGG\tLN:i:2\n\
        S\t3\t*\n\
        L\t1\t+\t2\t-\t3M\n\
        L\t2\t-\t3\t+\t*\n\
        P\tp1\t1+,2-,3+\t3M,0M\n\
        P\tp2\t3-\t*\n";

    #[test]
    fn parses_small_graph() {
        let g = parse_str(SMALL).unwrap();
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.nodes[0].sequence, b"ACGT");
        assert_eq!(g.nodes[2].sequence, b"*");
        assert_eq!(g.get_name_from_id(1), "2");
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.edges[0].overlap, 3);
        assert_eq!(g.edges[0].to_orient, Orientation::Reverse);
        assert_eq!(g.edges[1].overlap, 0);
        assert_eq!(
            step_names(&g, 0),
            vec![
                ("1".into(), Orientation::Forward),
                ("2".into(), Orientation::Reverse),
                ("3".into(), Orientation::Forward)
            ]
        );
        assert_eq!(g.paths[0].overlaps, vec![3, 0]);
        assert_eq!(g.paths[1].overlaps, Vec::<u32>::new());
    }

    #[test]
    fn crlf_and_missing_final_newline() {
        let text = SMALL.replace('\n', "\r\n");
        let g = parse_str(text.trim_end()).unwrap();
        assert_eq!(g, parse_str(SMALL).unwrap());
    }

    #[test]
    fn records_before_their_segments() {
        let text = "P\tp\tb+,a-\t*\nL\ta\t+\tb\t+\t0M\nS\ta\tA\nS\tb\tC\n";
        let g = parse_str(text).unwrap();
        assert_eq!(g.get_name_from_id(0), "a");
        assert_eq!(
            step_names(&g, 0),
            vec![
                ("b".into(), Orientation::Forward),
                ("a".into(), Orientation::Reverse)
            ]
        );
        assert_eq!((g.edges[0].from_node, g.edges[0].to_node), (0, 1));
    }

    #[test]
    fn non_canonical_numbers_are_names() {
        // Mixed names fall back to hashing; `007` and `7` are different segments.
        let g = parse_str("S\t7\tA\nS\t007\tC\nP\tp\t007+,7+\t*\n").unwrap();
        assert_eq!(step_names(&g, 0)[0].0, "007");
        assert_eq!(g.paths[0].steps[0].node_id(), 1);
        // With numeric names, a non-canonical reference does not resolve.
        assert!(err("S\t7\tA\nP\tp\t007+\t*\n").contains("undefined segment 007"));
    }

    #[test]
    fn skips_other_record_types() {
        let text =
            "# comment\nH\tVN:Z:1.1\nS\t1\tA\n\nW\tx\t0\tc\t0\t1\t>1\nC\t1\t+\t1\t+\t0\t1M\n";
        let g = parse_str(text).unwrap();
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.is_empty() && g.paths.is_empty());
    }

    #[test]
    fn empty_input() {
        let g = parse_str("").unwrap();
        assert!(g.nodes.is_empty() && g.edges.is_empty() && g.paths.is_empty());
    }

    #[test]
    fn reports_errors_with_line_numbers() {
        let cases = [
            ("S\t1\tA\nS\t1\tC\n", "line 2: duplicate segment name 1"),
            ("S\ta\tA\nS\ta\tC\n", "line 2: duplicate segment name a"),
            (
                "S\ta\tA\nL\ta\t+\ta\t+\t0M\n\nS\tb\tC\nS\ta\tG\n",
                "line 5: duplicate segment name a",
            ),
            (
                "S\t1\tA\nS\t2\tA\nH\nS\t2\tC\n",
                "line 4: duplicate segment name 2",
            ),
            ("S\t1\tAC GT\n", "line 1: segment 1 has an invalid sequence"),
            ("S\t1\n", "line 1: segment 1 has no sequence field"),
            (
                "S\t1\tA\nL\t1\t+\t2\t+\t0M\n",
                "line 2: reference to undefined segment 2",
            ),
            (
                "S\t1\tA\nL\t1\tx\t1\t+\t0M\n",
                "line 2: invalid orientation",
            ),
            (
                "S\t1\tA\nL\t1\t+\t1\t+\t5Q\n",
                "line 2: invalid CIGAR overlap",
            ),
            (
                "S\t1\tA\nL\t1\t+\t1\t+\n",
                "line 2: L line without an overlap",
            ),
            (
                "S\t1\tA\n\nP\tp\t1\t*\n",
                "line 3: path p: step \"1\" has no orientation",
            ),
            ("S\t1\tA\nP\tp\t1+,,1+\t*\n", "line 2: path p: empty step"),
            ("S\t1\tA\nP\tp\t1+\n", "line 2: path p has no overlap field"),
        ];
        for (text, expected) in cases {
            let message = err(text);
            assert!(message.contains(expected), "{text:?}: {message}");
        }
    }

    #[test]
    fn cigar_overlaps() {
        assert_eq!(parse_overlap(b"*"), Ok(0));
        assert_eq!(parse_overlap(b"10M"), Ok(10));
        assert_eq!(parse_overlap(b"3M2I4M"), Ok(7));
        assert!(parse_overlap(b"").is_err());
        assert!(parse_overlap(b"M").is_err());
        assert!(parse_overlap(b"12").is_err());
        assert!(parse_overlap(b"99999999999M").is_err());
    }

    #[test]
    fn tiny_blocks_match_default_blocks() {
        // Every block boundary falls inside a line; some lines exceed a block.
        let reference = parse_str(SMALL).unwrap();
        for block_size in [1, 2, 7, 16] {
            let g = parse_with_block_size(&mut Cursor::new(SMALL.as_bytes()), block_size).unwrap();
            assert_eq!(g, reference, "block size {block_size}");
        }
    }

    #[test]
    fn long_step_lists_are_split_in_pieces() {
        let list: Vec<u8> = (0..100)
            .flat_map(|i| format!("{i}+,").into_bytes())
            .collect();
        let list = &list[..list.len() - 1];
        for piece_size in [1, 5, 64, list.len()] {
            let pieces = split_at_commas(list, piece_size);
            assert_eq!(pieces.join(&b','), list, "piece size {piece_size}");
        }

        let mut text = String::new();
        for i in 0..300_000 {
            writeln!(text, "S\t{i}\tA").unwrap();
        }
        text.push_str("P\tp\t");
        let steps: Vec<String> = (0..300_000).map(|i| format!("{i}-")).collect();
        text.push_str(&steps.join(","));
        text.push_str("\t*\n");
        let g = parse_str(&text).unwrap();
        assert!(steps.join(",").len() > PATH_PIECE_SIZE);
        assert_eq!(g.paths[0].steps.len(), 300_000);
        assert!(
            g.paths[0]
                .steps
                .iter()
                .enumerate()
                .all(|(i, s)| s.node_id() == i && s.is_reverse())
        );
    }

    #[test]
    fn reads_from_current_position() {
        let mut cursor = Cursor::new(format!("ignored\n{SMALL}").into_bytes());
        cursor.set_position(8);
        assert_eq!(parse(&mut cursor).unwrap(), parse_str(SMALL).unwrap());
    }
}
