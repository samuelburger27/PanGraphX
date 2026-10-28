//! Colored de Bruijn graphs: the de Bruijn graph of the input paths, with
//! every path kept as a walk through its k-mers.
//!
//! The walks are never stored, since they hold one step per path base.
//! Construction makes two passes over the paths: the first collects the
//! distinct k-mers and edges, the second records only the first and last
//! k-mer of every gap-free segment (compaction pins segment ends to unitig
//! ends). Output walks the input paths once more and formats, collects or
//! translates each k-mer as it is rolled. Memory therefore grows with the
//! number of distinct k-mers and the input graph, not with path length.
//!
//! Every pass splits the paths into the same kind of chunks and reassembles
//! them in order (see [`walk_segments`]), so all passes agree on where a path
//! is split into segments.

use super::compaction::{self, SegmentEnds, Translator, UnitigPos, Unitigs};
use super::de_bruijn_graph::{
    DbgCore, DbgTables, DeBruijn, build_from_paths, edge_overlap, kmer_rank, unitigs_to_dto,
    with_core,
};
use super::extraction::{CHUNK_BASES, WindowSink};
use super::output::push_number;
use super::word::{KmerWord, MAX_SMALL_K, Spelled};
use crate::core::core_types::{Path, PathName, Step};
use crate::core::graph::CoreGraph;
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use crate::formats::gfa_format::orientation_byte;
use log::debug;
use std::io::Write;
use std::mem::take;

/// Bases per path chunk when formatting paths as GFA. A formatted step takes
/// about 10 bytes, and one chunk per thread is held before it is written, so
/// chunks are kept well below [`CHUNK_BASES`]: about 1.3 MiB of text each.
const FORMAT_CHUNK_BASES: usize = 1 << 17;

/// One gap-free piece of an input path that holds at least one k-mer. Each
/// segment becomes one path of the colored graph.
struct Segment {
    /// The input path's name, or `<name>.<i>` (1-based) when non-nucleotide
    /// bases split the path into several segments.
    name: PathName,
    /// First k-mer, as `(node id, reverse)`.
    first: (usize, bool),
    /// Last k-mer, as `(node id, reverse)`.
    last: (usize, bool),
    /// Number of k-mer occurrences.
    kmers: usize,
}

/// A de Bruijn graph that also keeps every input path as a walk through it.
pub struct ColoredDBG {
    pub dbg: DeBruijn,
    /// The nodes and paths of the input graph, walked again whenever the
    /// paths are needed.
    graph: CoreGraph,
    segments: Vec<Segment>,
    /// Bases per path chunk of every walk (small in tests, to put chunk
    /// boundaries everywhere).
    chunk_bases: usize,
}

impl ColoredDBG {
    /// Builds the colored de Bruijn graph of the paths of `graph`.
    ///
    /// Runs in two passes over the paths: the first collects the distinct
    /// k-mers and edges, the second finds the gap-free segments of each path.
    /// The walks themselves are produced on output.
    ///
    /// # Errors
    /// Returns an error if `k` is not in `1..=63` or the graph refers to
    /// missing nodes.
    pub fn from_directed_graph(graph: CoreGraphDTO, k: usize) -> PanResult<Self> {
        Self::with_chunk_bases(graph, k, CHUNK_BASES)
    }

    pub(crate) fn with_chunk_bases(
        graph: CoreGraphDTO,
        k: usize,
        chunk_bases: usize,
    ) -> PanResult<Self> {
        let mut lookup = DeBruijn::prepare(graph, k)?;
        // The input is kept to walk the paths again, so drop the rest of it.
        lookup.keep_only_paths();
        let (tables, segments) = if k <= MAX_SMALL_K {
            let core = build_from_paths::<u64>(&lookup, k);
            let segments = find_segments(&lookup, &core, k, chunk_bases)?;
            (DbgTables::Small(core), segments)
        } else {
            let core = build_from_paths::<u128>(&lookup, k);
            let segments = find_segments(&lookup, &core, k, chunk_bases)?;
            (DbgTables::Large(core), segments)
        };
        Ok(Self {
            dbg: DeBruijn::finish(k, tables),
            graph: lookup,
            segments,
            chunk_bases,
        })
    }

    /// Number of paths of the colored graph (gap-free segments of the input
    /// paths that hold a k-mer).
    #[must_use]
    pub const fn path_count(&self) -> usize {
        self.segments.len()
    }

    /// Names of the paths of the colored graph, in output order.
    pub fn path_names(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.segments.iter().map(|s| s.name.as_slice())
    }

    /// Writes the graph as GFA: one segment per k-mer, and every path as a
    /// walk over k-mers whose steps overlap by `k - 1`.
    ///
    /// The output is the same as writing the [`CoreGraphDTO`] of the graph as
    /// GFA, without holding that DTO or the walks in memory.
    ///
    /// # Errors
    /// Returns an error if writing fails.
    pub fn write_gfa(&self, writer: &mut dyn Write) -> PanResult<()> {
        with_core!(&self.dbg.tables, core => self.write_gfa_with(core, writer))
    }

    fn write_gfa_with<W: KmerWord>(
        &self,
        core: &DbgCore<W>,
        writer: &mut dyn Write,
    ) -> PanResult<()> {
        let k = self.dbg.k;
        core.write_segments_and_links(k, writer)?;
        let overlap = edge_overlap(k)?;
        let mut segments = self.segments.iter();
        // The segment being written and the k-mers written so far.
        let mut current: Option<(&Segment, usize)> = None;
        walk_segments(
            &self.graph,
            k,
            self.chunk_bases.min(FORMAT_CHUNK_BASES),
            || FormatBuilder::new(core),
            |event| {
                match event {
                    SegmentEvent::Run(run) => {
                        if run.continues {
                            writer.write_all(b",")?;
                        } else {
                            let segment = segments.next().ok_or_else(pass_mismatch)?;
                            writer.write_all(b"P\t")?;
                            writer.write_all(&segment.name)?;
                            writer.write_all(b"\t")?;
                            current = Some((segment, 0));
                        }
                        writer.write_all(&run.run.text)?;
                        let (_, written) = current.as_mut().ok_or_else(pass_mismatch)?;
                        *written += run.run.kmers;
                    }
                    SegmentEvent::End => {
                        let (segment, written) = current.take().ok_or_else(pass_mismatch)?;
                        if written != segment.kmers {
                            return Err(pass_mismatch());
                        }
                        write_overlaps(writer, written, overlap)?;
                    }
                }
                Ok(())
            },
        )?;
        if segments.next().is_some() {
            return Err(pass_mismatch());
        }
        Ok(())
    }

    /// Converts to a graph of unitigs, with every path translated to a walk
    /// over them. Path segments always start and end at unitig ends, so each
    /// walk spells exactly the sequence of its segment.
    ///
    /// # Errors
    /// Returns an error if the graph is too large for 32-bit unitig ids.
    pub fn into_compacted_dto(self) -> PanResult<CoreGraphDTO> {
        let Self {
            dbg,
            graph,
            segments,
            chunk_bases,
        } = self;
        let k = dbg.k;
        let overlap = edge_overlap(k)?;
        let ends: Vec<SegmentEnds> = segments
            .iter()
            .map(|s| SegmentEnds {
                first: s.first,
                last: s.last,
            })
            .collect();
        let (unitigs, paths) = with_core!(dbg.tables, core => {
            let (unitigs, kmers) = compaction::compact(core, k, &ends)?;
            drop(ends);
            let paths = translate_paths(&graph, &kmers, &unitigs, k, chunk_bases, &segments, overlap)?;
            (unitigs, paths)
        });
        drop(graph);
        unitigs_to_dto(unitigs, k, paths)
    }
}

/// Conversion to a DTO with one node per k-mer. This materializes every walk;
/// [`ColoredDBG::write_gfa`] writes the same graph without doing so.
impl TryFrom<ColoredDBG> for CoreGraphDTO {
    type Error = PanGraphXError;

    fn try_from(colored_dbg: ColoredDBG) -> PanResult<Self> {
        let ColoredDBG {
            dbg,
            graph,
            segments,
            chunk_bases,
        } = colored_dbg;
        let k = dbg.k;
        let overlap = edge_overlap(k)?;
        let paths = with_core!(&dbg.tables, core => {
            collect_paths(&graph, core, k, chunk_bases, &segments, overlap)?
        });
        drop(graph);
        let (nodes, edges) = with_core!(dbg.tables, core => core.into_dto_parts(k)?);
        Ok(Self {
            nodes,
            edges,
            paths,
            node_name_map: None,
        })
    }
}

fn pass_mismatch() -> PanGraphXError {
    PanGraphXError::DeBruijn("a colored path walk does not match the segments found for it".into())
}

/// Writes the overlap field of a path line with `steps` steps that all
/// overlap by `overlap` (`*` when there is nothing to record), and ends the
/// line.
fn write_overlaps(writer: &mut dyn Write, steps: usize, overlap: u32) -> PanResult<()> {
    writer.write_all(b"\t")?;
    if steps <= 1 || overlap == 0 {
        writer.write_all(b"*\n")?;
        return Ok(());
    }
    let mut token = Vec::new();
    push_number(&mut token, overlap);
    token.push(b'M');
    writer.write_all(&token)?;
    // The other `steps - 2` overlaps, written in blocks of `,<overlap>M`.
    let mut block = Vec::new();
    let mut remaining = steps - 2;
    while remaining > 0 {
        let n = remaining.min(4096);
        if block.len() != n * (token.len() + 1) {
            block.clear();
            for _ in 0..n {
                block.push(b',');
                block.extend_from_slice(&token);
            }
        }
        writer.write_all(&block)?;
        remaining -= n;
    }
    writer.write_all(b"\n")?;
    Ok(())
}

// ===== Walking the segments of the paths =====

/// A chunk walk, split at gaps (non-nucleotide bases): runs of consecutive
/// k-mers, each followed by a gap unless it reaches the end of the chunk.
enum Piece<R> {
    Run(R),
    Gap,
}

/// Builds the run of consecutive k-mers a chunk walk is in.
trait RunBuilder {
    type Word: KmerWord;
    type Run;
    /// Adds the next k-mer of the run.
    fn push(&mut self, kmer: Spelled<Self::Word>) -> PanResult<()>;
    /// Ends the current run; `None` if it holds no k-mer.
    fn take(&mut self) -> PanResult<Option<Self::Run>>;
}

/// A [`WindowSink`] that splits a chunk walk into [`Piece`]s.
struct PieceSink<B: RunBuilder> {
    builder: B,
    pieces: Vec<Piece<B::Run>>,
    error: Option<PanGraphXError>,
}

impl<B: RunBuilder> PieceSink<B> {
    const fn new(builder: B) -> Self {
        Self {
            builder,
            pieces: Vec::new(),
            error: None,
        }
    }

    /// Ends the current run, returning whether it held a k-mer.
    fn close_run(&mut self) -> bool {
        match self.builder.take() {
            Ok(Some(run)) => {
                self.pieces.push(Piece::Run(run));
                true
            }
            Ok(None) => false,
            Err(e) => {
                self.error.get_or_insert(e);
                false
            }
        }
    }

    fn finish(mut self) -> PanResult<Vec<Piece<B::Run>>> {
        self.close_run();
        self.error.map_or(Ok(self.pieces), Err)
    }
}

impl<B: RunBuilder> WindowSink<B::Word> for PieceSink<B> {
    #[inline]
    fn kmer(&mut self, kmer: Spelled<B::Word>) {
        if self.error.is_none()
            && let Err(e) = self.builder.push(kmer)
        {
            self.error = Some(e);
        }
    }

    #[inline]
    fn edge(&mut self, _edge: B::Word) {}

    fn gap(&mut self) {
        if self.close_run() {
            self.pieces.push(Piece::Gap);
        }
    }
}

/// A run of k-mers of a segment, in path order.
struct SegmentRun<'p, R> {
    /// Index of the path in [`CoreGraph::sorted_paths`].
    path_index: usize,
    path: &'p Path,
    /// Whether the run continues a segment begun by an earlier run (of an
    /// earlier chunk of the same path).
    continues: bool,
    run: R,
}

enum SegmentEvent<'p, R> {
    Run(SegmentRun<'p, R>),
    /// The segment of the previous runs is complete.
    End,
}

/// Walks the paths of `graph` in name order, chunk by chunk with a fresh run
/// builder per chunk, and reports the runs of every gap-free segment to
/// `on_event` in path order, each segment followed by [`SegmentEvent::End`].
fn walk_segments<B>(
    graph: &CoreGraph,
    k: usize,
    chunk_bases: usize,
    new_builder: impl Fn() -> B + Sync,
    mut on_event: impl FnMut(SegmentEvent<'_, B::Run>) -> PanResult<()>,
) -> PanResult<()>
where
    B: RunBuilder + Send,
    B::Run: Send,
{
    let paths = graph.sorted_paths();
    let mut open = false;
    graph.walk_chunks_ordered(
        &paths,
        chunk_bases,
        k,
        || PieceSink::new(new_builder()),
        |chunk, sink, last_of_path| {
            for piece in sink.finish()? {
                match piece {
                    Piece::Run(run) => {
                        on_event(SegmentEvent::Run(SegmentRun {
                            path_index: chunk.path_index,
                            path: chunk.path,
                            continues: open,
                            run,
                        }))?;
                        open = true;
                    }
                    Piece::Gap => {
                        if open {
                            on_event(SegmentEvent::End)?;
                            open = false;
                        }
                    }
                }
            }
            if last_of_path && open {
                on_event(SegmentEvent::End)?;
                open = false;
            }
            Ok(())
        },
    )
}

/// Finds the gap-free segments of the paths of `graph`, with their end
/// k-mers and names.
fn find_segments<W: KmerWord>(
    graph: &CoreGraph,
    core: &DbgCore<W>,
    k: usize,
    chunk_bases: usize,
) -> PanResult<Vec<Segment>> {
    let mut segments: Vec<Segment> = Vec::new();
    // Index of the current path and of its first segment.
    let mut path: Option<usize> = None;
    let mut path_start = 0;
    let mut paths_with_kmers = 0;
    walk_segments(
        graph,
        k,
        chunk_bases,
        || EndsBuilder::new(core),
        |event| {
            let SegmentEvent::Run(run) = event else {
                return Ok(());
            };
            if run.continues {
                let segment = segments.last_mut().ok_or_else(pass_mismatch)?;
                segment.last = run.run.last;
                segment.kmers += run.run.kmers;
                return Ok(());
            }
            if path != Some(run.path_index) {
                name_segments(&mut segments[path_start..]);
                path_start = segments.len();
                path = Some(run.path_index);
                paths_with_kmers += 1;
            }
            segments.push(Segment {
                name: run.path.name.clone(),
                first: run.run.first,
                last: run.run.last,
                kmers: run.run.kmers,
            });
            Ok(())
        },
    )?;
    name_segments(&mut segments[path_start..]);
    let skipped = graph.path_count().saturating_sub(paths_with_kmers);
    if skipped > 0 {
        debug!("{skipped} paths have no k-mers and are left out of the colored graph");
    }
    Ok(segments)
}

/// Names the segments of one path `<name>.<i>` (1-based) when there are
/// several.
fn name_segments(segments: &mut [Segment]) {
    if segments.len() > 1 {
        for (i, segment) in segments.iter_mut().enumerate() {
            segment.name.push(b'.');
            push_number(&mut segment.name, i + 1);
        }
    }
}

/// Collects the walks of the segments over k-mer nodes.
fn collect_paths<W: KmerWord>(
    graph: &CoreGraph,
    core: &DbgCore<W>,
    k: usize,
    chunk_bases: usize,
    segments: &[Segment],
    overlap: u32,
) -> PanResult<Vec<Path>> {
    let mut paths = Vec::with_capacity(segments.len());
    let mut steps: Vec<Step> = Vec::new();
    walk_segments(
        graph,
        k,
        chunk_bases,
        || StepBuilder::new(core),
        |event| {
            match event {
                SegmentEvent::Run(run) => steps.extend(run.run),
                SegmentEvent::End => {
                    let segment = segments.get(paths.len()).ok_or_else(pass_mismatch)?;
                    if steps.len() != segment.kmers {
                        return Err(pass_mismatch());
                    }
                    paths.push(Path {
                        name: segment.name.clone(),
                        overlaps: vec![overlap; steps.len().saturating_sub(1)],
                        steps: take(&mut steps),
                    });
                }
            }
            Ok(())
        },
    )?;
    if paths.len() == segments.len() {
        Ok(paths)
    } else {
        Err(pass_mismatch())
    }
}

/// Translates the walks of the segments onto the unitigs.
fn translate_paths<W: KmerWord>(
    graph: &CoreGraph,
    kmers: &[W],
    unitigs: &Unitigs,
    k: usize,
    chunk_bases: usize,
    segments: &[Segment],
    overlap: u32,
) -> PanResult<Vec<Path>> {
    let mut paths = Vec::with_capacity(segments.len());
    let mut steps: Vec<Step> = Vec::new();
    let mut kmer_count = 0;
    // Position of the last k-mer of the previous run.
    let mut last: Option<UnitigPos> = None;
    walk_segments(
        graph,
        k,
        chunk_bases,
        || TranslateBuilder::new(kmers, unitigs),
        |event| {
            match event {
                SegmentEvent::Run(run) => {
                    let run = run.run;
                    // A run starts a new unitig step unless it carries on the
                    // unitig traversal the previous run ended in.
                    let skip = usize::from(last.is_some_and(|l| l.continues_to(run.first)));
                    steps.extend(run.steps.into_iter().skip(skip));
                    last = Some(run.last);
                    kmer_count += run.kmers;
                }
                SegmentEvent::End => {
                    let segment = segments.get(paths.len()).ok_or_else(pass_mismatch)?;
                    if kmer_count != segment.kmers {
                        return Err(pass_mismatch());
                    }
                    paths.push(Path {
                        name: segment.name.clone(),
                        overlaps: vec![overlap; steps.len().saturating_sub(1)],
                        steps: take(&mut steps),
                    });
                    last = None;
                    kmer_count = 0;
                }
            }
            Ok(())
        },
    )?;
    if paths.len() == segments.len() {
        Ok(paths)
    } else {
        Err(pass_mismatch())
    }
}

// ===== Run builders =====

/// First and last k-mer of a run, and its length.
struct EndsRun {
    first: (usize, bool),
    last: (usize, bool),
    kmers: usize,
}

/// Records only the ends of a run; node ids are looked up for those alone.
struct EndsBuilder<'c, W> {
    core: &'c DbgCore<W>,
    first: Option<Spelled<W>>,
    last: Option<Spelled<W>>,
    kmers: usize,
}

impl<'c, W> EndsBuilder<'c, W> {
    const fn new(core: &'c DbgCore<W>) -> Self {
        Self {
            core,
            first: None,
            last: None,
            kmers: 0,
        }
    }
}

impl<W: KmerWord> RunBuilder for EndsBuilder<'_, W> {
    type Word = W;
    type Run = EndsRun;

    #[inline]
    fn push(&mut self, kmer: Spelled<W>) -> PanResult<()> {
        self.first.get_or_insert(kmer);
        self.last = Some(kmer);
        self.kmers += 1;
        Ok(())
    }

    fn take(&mut self) -> PanResult<Option<EndsRun>> {
        let (Some(first), Some(last)) = (self.first.take(), self.last.take()) else {
            return Ok(None);
        };
        let oriented = |kmer: Spelled<W>| -> PanResult<(usize, bool)> {
            Ok((self.core.id_of(kmer.canonical())?, kmer.is_reverse()))
        };
        Ok(Some(EndsRun {
            first: oriented(first)?,
            last: oriented(last)?,
            kmers: take(&mut self.kmers),
        }))
    }
}

/// A run formatted as GFA path steps (`<id><+|->`, comma-separated).
struct FormatRun {
    text: Vec<u8>,
    kmers: usize,
}

struct FormatBuilder<'c, W> {
    core: &'c DbgCore<W>,
    text: Vec<u8>,
    kmers: usize,
}

impl<'c, W> FormatBuilder<'c, W> {
    const fn new(core: &'c DbgCore<W>) -> Self {
        Self {
            core,
            text: Vec::new(),
            kmers: 0,
        }
    }
}

impl<W: KmerWord> RunBuilder for FormatBuilder<'_, W> {
    type Word = W;
    type Run = FormatRun;

    #[inline]
    fn push(&mut self, kmer: Spelled<W>) -> PanResult<()> {
        if self.kmers > 0 {
            self.text.push(b',');
        }
        push_number(&mut self.text, self.core.id_of(kmer.canonical())?);
        self.text.push(orientation_byte(kmer.orientation()));
        self.kmers += 1;
        Ok(())
    }

    fn take(&mut self) -> PanResult<Option<FormatRun>> {
        if self.kmers == 0 {
            return Ok(None);
        }
        Ok(Some(FormatRun {
            text: take(&mut self.text),
            kmers: take(&mut self.kmers),
        }))
    }
}

/// Collects a run as path steps over k-mer nodes.
struct StepBuilder<'c, W> {
    core: &'c DbgCore<W>,
    steps: Vec<Step>,
}

impl<'c, W> StepBuilder<'c, W> {
    const fn new(core: &'c DbgCore<W>) -> Self {
        Self {
            core,
            steps: Vec::new(),
        }
    }
}

impl<W: KmerWord> RunBuilder for StepBuilder<'_, W> {
    type Word = W;
    type Run = Vec<Step>;

    #[inline]
    fn push(&mut self, kmer: Spelled<W>) -> PanResult<()> {
        let id = self.core.id_of(kmer.canonical())?;
        self.steps.push(Step::new(id, kmer.orientation())?);
        Ok(())
    }

    fn take(&mut self) -> PanResult<Option<Vec<Step>>> {
        Ok((!self.steps.is_empty()).then(|| take(&mut self.steps)))
    }
}

/// A run translated onto unitigs, with the unitig positions of its first and
/// last k-mer so that runs split by a chunk boundary can be joined.
struct TranslateRun {
    steps: Vec<Step>,
    first: UnitigPos,
    last: UnitigPos,
    kmers: usize,
}

struct TranslateBuilder<'u, W> {
    kmers: &'u [W],
    translator: Translator<'u>,
    steps: Vec<Step>,
    first: Option<UnitigPos>,
    last: Option<UnitigPos>,
    count: usize,
}

impl<'u, W> TranslateBuilder<'u, W> {
    const fn new(kmers: &'u [W], unitigs: &'u Unitigs) -> Self {
        Self {
            kmers,
            translator: Translator::new(unitigs),
            steps: Vec::new(),
            first: None,
            last: None,
            count: 0,
        }
    }
}

impl<W: KmerWord> RunBuilder for TranslateBuilder<'_, W> {
    type Word = W;
    type Run = TranslateRun;

    #[inline]
    fn push(&mut self, kmer: Spelled<W>) -> PanResult<()> {
        let id = kmer_rank(self.kmers, kmer.canonical())?;
        let pos = self
            .translator
            .push(id, kmer.is_reverse(), &mut self.steps)?;
        self.first.get_or_insert(pos);
        self.last = Some(pos);
        self.count += 1;
        Ok(())
    }

    fn take(&mut self) -> PanResult<Option<TranslateRun>> {
        let (Some(first), Some(last)) = (self.first.take(), self.last.take()) else {
            return Ok(None);
        };
        self.translator.reset();
        Ok(Some(TranslateRun {
            steps: take(&mut self.steps),
            first,
            last,
            kmers: take(&mut self.count),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::de_bruijn_conversion::de_bruijn_graph::tests::{canon, graph};
    use crate::formats::GFACodec;
    use crate::traits::GraphSerializer;
    use crate::{Kmer, Orientation};

    fn oriented(seq: &[u8], o: Orientation) -> Vec<u8> {
        if o == Orientation::Forward {
            seq.to_vec()
        } else {
            Kmer::from_bases(seq).rev_comp().to_bytes()
        }
    }

    /// The sequence a DTO path spells, merging consecutive nodes by `k - 1`.
    fn spell(dto: &CoreGraphDTO, path: &Path, k: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, step) in path.steps.iter().enumerate() {
            let seq = oriented(&dto.nodes[step.node_id()].sequence, step.orientation());
            out.extend_from_slice(if i == 0 { &seq } else { &seq[k - 1..] });
        }
        out
    }

    /// The sequence an input path spells.
    fn input_spell(g: &CoreGraphDTO, path: &Path) -> Vec<u8> {
        path.steps
            .iter()
            .flat_map(|s| oriented(&g.nodes[s.node_id()].sequence, s.orientation()))
            .collect()
    }

    #[test]
    fn test_colored_paths() {
        let g = graph(
            &[b"ACGTA", b"TTGCA"],
            &[],
            &[("path1", &[(0, false)]), ("path2", &[(1, false)])],
        );
        let colored = ColoredDBG::from_directed_graph(g, 4).unwrap();
        assert_eq!(colored.path_count(), 2);
        assert_eq!(colored.path_names().next(), Some(b"path1".as_slice()));
        let dto = CoreGraphDTO::try_from(colored).unwrap();
        assert_eq!(dto.paths[0].steps.len(), 2);
        assert_eq!(spell(&dto, &dto.paths[0], 4), b"ACGTA");
        assert_eq!(spell(&dto, &dto.paths[1], 4), b"TTGCA");
    }

    #[test]
    fn test_many_paths_keep_names() {
        // Used to panic for 10 or more paths.
        let seqs: Vec<&[u8]> = vec![b"ACGTACGGT"; 12];
        let names: Vec<String> = (0..12).map(|i| format!("sample{i:02}")).collect();
        let steps: Vec<Vec<(usize, bool)>> = (0..12).map(|i| vec![(i, false)]).collect();
        let paths: Vec<(&str, &[(usize, bool)])> = names
            .iter()
            .zip(&steps)
            .map(|(n, s)| (n.as_str(), s.as_slice()))
            .collect();
        let colored = ColoredDBG::from_directed_graph(graph(&seqs, &[], &paths), 5).unwrap();
        let got: Vec<String> = colored
            .path_names()
            .map(|name| String::from_utf8(name.to_vec()).unwrap())
            .collect();
        assert_eq!(got, names);
    }

    #[test]
    fn test_gap_splits_path_into_segments() {
        let g = graph(&[b"ACGTANNCCGTA"], &[], &[("p", &[(0, false)])]);
        let colored = ColoredDBG::from_directed_graph(g, 3).unwrap();
        let names: Vec<&[u8]> = colored.path_names().collect();
        assert_eq!(names, [b"p.1".as_slice(), b"p.2".as_slice()]);
        let dto = CoreGraphDTO::try_from(colored).unwrap();
        assert_eq!(spell(&dto, &dto.paths[0], 3), b"ACGTA");
        assert_eq!(spell(&dto, &dto.paths[1], 3), b"CCGTA");
    }

    /// Haplotypes with shared sequence, a SNP, an indel, a reverse step and
    /// a gap.
    fn haplotype_graph() -> CoreGraphDTO {
        graph(
            &[
                b"TTGACCAGTAGG",
                b"A",
                b"C",
                b"CATGGATTCAGGCT",
                b"GGTTNACG",
                b"AGCCTGAATC",
            ],
            &[],
            &[
                ("h1", &[(0, false), (1, false), (3, false)]),
                ("h2", &[(0, false), (2, false), (3, false), (4, false)]),
                ("h3", &[(0, false), (3, false)]),
                ("h4", &[(5, true), (0, false)]),
            ],
        )
    }

    #[test]
    fn test_compacted_paths_spell_input() {
        for k in [3, 4, 5, 7] {
            let g = haplotype_graph();
            let colored = ColoredDBG::from_directed_graph(g.clone(), k).unwrap();
            let names: Vec<PathName> = colored.path_names().map(<[u8]>::to_vec).collect();
            let dto = colored.into_compacted_dto().unwrap();
            assert!(dto.nodes.len() > 1);
            for (path, name) in dto.paths.iter().zip(&names) {
                assert_eq!(&path.name, name);
                let base = name.split(|&b| b == b'.').next().unwrap();
                let input = g.paths.iter().find(|p| p.name == base).unwrap();
                let full = input_spell(&g, input);
                // Each segment is one of the N-free pieces of the input.
                let pieces: Vec<&[u8]> = full
                    .split(|&b| b == b'N')
                    .filter(|s| s.len() >= k)
                    .collect();
                let spelled = spell(&dto, path, k);
                assert!(
                    pieces.contains(&spelled.as_slice()),
                    "k={k} path {}: {} not in {:?}",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(&spelled),
                    pieces
                        .iter()
                        .map(|p| String::from_utf8_lossy(p))
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn test_compaction_shrinks_graph() {
        let g = haplotype_graph();
        let k = 5;
        let plain =
            CoreGraphDTO::try_from(ColoredDBG::from_directed_graph(g.clone(), k).unwrap()).unwrap();
        let compact = ColoredDBG::from_directed_graph(g, k)
            .unwrap()
            .into_compacted_dto()
            .unwrap();
        assert!(compact.nodes.len() < plain.nodes.len());
        // Every k-mer of the plain graph appears in some unitig.
        let unitig_kmers: std::collections::HashSet<String> = compact
            .nodes
            .iter()
            .flat_map(|n| n.sequence.windows(k).map(canon).collect::<Vec<_>>())
            .collect();
        for node in plain.nodes.iter() {
            assert!(unitig_kmers.contains(&canon(&node.sequence)));
        }
        assert_eq!(unitig_kmers.len(), plain.nodes.len());
    }

    /// GFA bytes of the uncompacted graph (streamed and via the DTO) and of
    /// the compacted graph, with walks split into chunks of `chunk_bases`.
    fn outputs(g: &CoreGraphDTO, k: usize, chunk_bases: usize) -> [Vec<u8>; 3] {
        let build = || ColoredDBG::with_chunk_bases(g.clone(), k, chunk_bases).unwrap();
        let mut streamed = Vec::new();
        build().write_gfa(&mut streamed).unwrap();
        let gfa = |dto: &CoreGraphDTO| {
            let mut out = Vec::new();
            GFACodec.serialize(dto, &mut out).unwrap();
            out
        };
        let dto = gfa(&CoreGraphDTO::try_from(build()).unwrap());
        let compacted = gfa(&build().into_compacted_dto().unwrap());
        [streamed, dto, compacted]
    }

    /// Paths with a gap at a node boundary, palindromes (`ACGT`), a cycle
    /// that walks the same unitig twice and reverse steps.
    fn chunked_graph() -> CoreGraphDTO {
        graph(
            &[
                b"ACGTACGTTGCA",
                b"NN",
                b"GGATCCATTAC",
                b"ACGTACGTTGCA",
                b"T",
                b"CCCAAATTTGGG",
            ],
            &[],
            &[
                (
                    "a",
                    &[
                        (0, false),
                        (1, false),
                        (2, false),
                        (3, false),
                        (0, true),
                        (4, false),
                    ],
                ),
                ("b", &[(5, false), (5, false), (5, true), (2, true)]),
                ("c", &[(1, false)]),
                ("d", &[(4, false), (4, false)]),
            ],
        )
    }

    #[test]
    fn test_streamed_gfa_matches_dto() {
        for k in [1, 2, 3, 5, 31, 33] {
            for g in [haplotype_graph(), chunked_graph()] {
                let [streamed, dto, _] = outputs(&g, k, CHUNK_BASES);
                assert!(
                    streamed == dto,
                    "k={k}:\n{}\n---\n{}",
                    String::from_utf8_lossy(&dto),
                    String::from_utf8_lossy(&streamed)
                );
            }
        }
    }

    #[test]
    fn test_output_independent_of_chunk_size() {
        for k in [2, 3, 4, 5, 7] {
            for g in [haplotype_graph(), chunked_graph()] {
                let expected = outputs(&g, k, CHUNK_BASES);
                for chunk_bases in [1, 2, 3, 5, 8] {
                    let actual = outputs(&g, k, chunk_bases);
                    for (what, (e, a)) in ["streamed", "dto", "compacted"]
                        .iter()
                        .zip(expected.iter().zip(&actual))
                    {
                        assert!(
                            e == a,
                            "k={k}, chunk_bases={chunk_bases}, {what}:\n{}\n---\n{}",
                            String::from_utf8_lossy(e),
                            String::from_utf8_lossy(a)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_path_without_kmers_is_left_out() {
        let g = chunked_graph();
        let colored = ColoredDBG::from_directed_graph(g, 3).unwrap();
        let names: Vec<&[u8]> = colored.path_names().collect();
        // `c` is only `NN`, and `d` (`TT`) is shorter than k.
        assert_eq!(names, [b"a.1".as_slice(), b"a.2", b"b"]);
    }
}
