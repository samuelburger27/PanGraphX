use super::de_bruijn_graph::{DbgCore, DbgTables, DeBruijn, build_from_paths, with_core};
use super::extraction::WindowSink;
use super::word::{KmerWord, MAX_SMALL_K, Spelled};
use crate::core::core_types::{NodeId, Orientation, Path, PathName, Step};
use crate::core::graph::CoreGraph;
use crate::core::graph_dto::CoreGraphDTO;
use crate::error::{PanGraphXError, PanResult};
use log::debug;
use rayon::prelude::*;
use std::mem::take;

/// Marks a gap (non-nucleotide bases) in a packed step sequence.
const GAP: u32 = u32::MAX;
/// Largest node id a packed step can hold without colliding with [`GAP`].
const MAX_PACKED_ID: usize = (1 << 31) - 2;

#[inline]
#[allow(clippy::cast_possible_truncation)]
const fn pack(id: usize, reverse: bool) -> u32 {
    // Node ids are checked against MAX_PACKED_ID before packing.
    ((id << 1) | reverse as usize) as u32
}

#[inline]
fn unpack(step: u32) -> (usize, bool) {
    // u32 always fits in usize on supported targets.
    (
        usize::try_from(step >> 1).unwrap_or(usize::MAX),
        step & 1 == 1,
    )
}

/// The walk of one input path (or of one gap-free segment of it) through the
/// de Bruijn graph: one step per k-mer occurrence.
pub struct ColorPath {
    /// Name of the input path; segments of a path split by non-nucleotide
    /// bases are named `<name>.<i>` (1-based).
    pub name: PathName,
    /// Packed `node << 1 | reverse` steps.
    steps: Vec<u32>,
}

impl ColorPath {
    /// Number of steps (k-mer occurrences).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.steps.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The steps as `(node id, orientation)`.
    pub fn steps(&self) -> impl Iterator<Item = (NodeId, Orientation)> + '_ {
        self.raw_steps()
            .map(|(id, reverse)| (id, Orientation::from(reverse)))
    }

    fn raw_steps(&self) -> impl Iterator<Item = (usize, bool)> + '_ {
        self.steps.iter().map(|&s| unpack(s))
    }
}

/// A de Bruijn graph that also keeps every input path as a walk through it.
pub struct ColoredDBG {
    pub dbg: DeBruijn,
    pub paths: Vec<ColorPath>,
}

impl ColoredDBG {
    /// Builds the colored de Bruijn graph of the paths of `graph`.
    ///
    /// Runs in two passes over the paths: the first collects the distinct
    /// k-mers and edges, the second re-rolls each path and records its walk as
    /// node ids.
    ///
    /// # Errors
    /// Returns an error if `k` is not in `1..=63`, the graph refers to
    /// missing nodes, or the graph has more than 2^31 - 1 k-mers.
    pub fn from_directed_graph(graph: CoreGraphDTO, k: usize) -> PanResult<Self> {
        let lookup = DeBruijn::prepare(graph, k)?;
        let (tables, paths) = if k <= MAX_SMALL_K {
            let (core, paths) = build::<u64>(&lookup, k)?;
            (DbgTables::Small(core), paths)
        } else {
            let (core, paths) = build::<u128>(&lookup, k)?;
            (DbgTables::Large(core), paths)
        };
        Ok(Self {
            dbg: DeBruijn::finish(k, tables),
            paths,
        })
    }
}

/// Both passes of the colored construction.
fn build<W: KmerWord>(graph: &CoreGraph, k: usize) -> PanResult<(DbgCore<W>, Vec<ColorPath>)> {
    let core = build_from_paths::<W>(graph, k);
    if core.kmers.len() > MAX_PACKED_ID + 1 {
        return Err(PanGraphXError::DeBruijn(format!(
            "colored de Bruijn graph supports at most {} k-mers, found {}",
            MAX_PACKED_ID + 1,
            core.kmers.len()
        )));
    }

    let paths = graph.sorted_paths();
    let chunks = graph.path_chunks(&paths);
    let chunk_steps = chunks
        .par_iter()
        .map(|chunk| {
            let mut sink = StepSink {
                core: &core,
                steps: Vec::new(),
                since_gap: false,
                error: None,
            };
            graph.walk_path_chunk(chunk.path, chunk.steps.clone(), k, &mut sink);
            sink.finish()
        })
        .collect::<PanResult<Vec<Vec<u32>>>>()?;

    // Stitch the chunks of each path back together, splitting at gaps.
    let mut colored = Vec::new();
    let mut segments: Vec<Vec<u32>> = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let mut iter = chunks.iter().zip(chunk_steps).peekable();
    while let Some((chunk, steps)) = iter.next() {
        for step in steps {
            if step != GAP {
                current.push(step);
            } else if !current.is_empty() {
                segments.push(take(&mut current));
            }
        }
        let path_done = iter
            .peek()
            .is_none_or(|(next, _)| next.path_index != chunk.path_index);
        if path_done {
            if !current.is_empty() {
                segments.push(take(&mut current));
            }
            if segments.is_empty() {
                debug!(
                    "path {} has no k-mers, it is left out of the colored graph",
                    String::from_utf8_lossy(&chunk.path.name)
                );
            }
            push_segments(&mut colored, &chunk.path.name, take(&mut segments));
        }
    }
    Ok((core, colored))
}

/// Adds the segments of one path, naming them after it.
fn push_segments(out: &mut Vec<ColorPath>, name: &PathName, segments: Vec<Vec<u32>>) {
    let single = segments.len() == 1;
    for (i, mut steps) in segments.into_iter().enumerate() {
        steps.shrink_to_fit();
        let mut name = name.clone();
        if !single {
            name.push(b'.');
            name.extend((i + 1).to_string().bytes());
        }
        out.push(ColorPath { name, steps });
    }
}

/// Records the walk of a path chunk as packed node steps.
struct StepSink<'a, W> {
    core: &'a DbgCore<W>,
    steps: Vec<u32>,
    /// Whether a step was recorded since the last gap marker.
    since_gap: bool,
    error: Option<PanGraphXError>,
}

impl<W> StepSink<'_, W> {
    fn finish(self) -> PanResult<Vec<u32>> {
        self.error.map_or(Ok(self.steps), Err)
    }
}

impl<W: KmerWord> WindowSink<W> for StepSink<'_, W> {
    #[inline]
    fn kmer(&mut self, kmer: Spelled<W>) {
        match self.core.id_of(kmer.canonical()) {
            Ok(id) => {
                self.steps.push(pack(id, kmer.is_reverse()));
                self.since_gap = true;
            }
            Err(e) => {
                self.error.get_or_insert(e);
            }
        }
    }

    #[inline]
    fn edge(&mut self, _edge: W) {}

    fn gap(&mut self) {
        if self.since_gap {
            self.steps.push(GAP);
            self.since_gap = false;
        }
    }
}

/// Conversion from `ColoredDBG` to `CoreGraph`
impl TryFrom<ColoredDBG> for CoreGraphDTO {
    type Error = PanGraphXError;

    fn try_from(colored_dbg: ColoredDBG) -> PanResult<Self> {
        let k = colored_dbg.dbg.k;
        let (nodes, edges) = with_core!(colored_dbg.dbg.tables, core => core.into_dto_parts(k)?);
        // Consecutive steps overlap by k - 1, as recorded on the edges; the
        // per-step overlaps are left unspecified ("*" in GFA).
        let paths = colored_dbg
            .paths
            .into_par_iter()
            .map(|p| Path {
                steps: p
                    .steps()
                    .map(|(node_id, orientation)| Step {
                        node_id,
                        orientation,
                    })
                    .collect(),
                name: p.name,
                overlaps: Vec::new(),
            })
            .collect();
        Ok(Self {
            nodes,
            edges,
            paths,
            node_name_map: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::de_bruijn_conversion::de_bruijn_graph::tests::graph;
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
            let seq = oriented(&dto.nodes[step.node_id].sequence, step.orientation);
            out.extend_from_slice(if i == 0 { &seq } else { &seq[k - 1..] });
        }
        out
    }

    #[test]
    fn test_colored_paths() {
        let g = graph(
            &[b"ACGTA", b"TTGCA"],
            &[],
            &[("path1", &[(0, false)]), ("path2", &[(1, false)])],
        );
        let colored = ColoredDBG::from_directed_graph(g, 4).unwrap();
        assert_eq!(colored.paths.len(), 2);
        assert_eq!(colored.paths[0].name, b"path1");
        assert_eq!(colored.paths[0].len(), 2);
        let dto = CoreGraphDTO::try_from(colored).unwrap();
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
            .paths
            .iter()
            .map(|p| String::from_utf8(p.name.clone()).unwrap())
            .collect();
        assert_eq!(got, names);
    }

    #[test]
    fn test_gap_splits_path_into_segments() {
        let g = graph(&[b"ACGTANNCCGTA"], &[], &[("p", &[(0, false)])]);
        let colored = ColoredDBG::from_directed_graph(g, 3).unwrap();
        let names: Vec<&[u8]> = colored.paths.iter().map(|p| p.name.as_slice()).collect();
        assert_eq!(names, [b"p.1".as_slice(), b"p.2".as_slice()]);
        let dto = CoreGraphDTO::try_from(colored).unwrap();
        assert_eq!(spell(&dto, &dto.paths[0], 3), b"ACGTA");
        assert_eq!(spell(&dto, &dto.paths[1], 3), b"CCGTA");
    }
}
