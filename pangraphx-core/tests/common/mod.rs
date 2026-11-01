//! Shared helpers for the integration tests: fixture paths, in-memory round
//! trips and one canonical form for comparing graphs across formats.
//!
//! Every format keeps a different subset of a graph (see [`Fidelity`]), so a
//! comparison projects *both* sides onto what the format can represent and
//! then compares the projections. Edges are always compared in canonical
//! orientation, since `a+ -> b+` and its mirror `b- -> a-` are the same link.
#![allow(dead_code)]

pub mod tools;

use pangraphx_core::{CoreGraphDTO, GraphFormat, NodeId, Orientation};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// How node labels are derived before comparing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Names {
    /// The node name, as stored in the graph.
    Kept,
    /// The name if every name is a distinct, positive, canonically written
    /// integer; otherwise `1..=n` in node order. This is how the VG and ODGI
    /// writers choose node ids.
    Renumbered,
    /// The sequence itself: for tools that renumber nodes in an order we
    /// cannot predict. Segments then compare as a multiset.
    BySequence,
}

/// What a format keeps of a graph.
#[allow(clippy::struct_excessive_bools)] // one flag per kept feature
#[derive(Clone, Copy, Debug)]
pub struct Fidelity {
    pub names: Names,
    /// Overlaps of links.
    pub overlaps: bool,
    /// Overlaps between consecutive path steps.
    pub path_overlaps: bool,
    /// Links that no path traverses.
    pub off_path_edges: bool,
    pub paths: bool,
    /// Repeated links; otherwise every link counts once.
    pub multi_edges: bool,
}

impl Fidelity {
    pub const GFA: Self = Self {
        names: Names::Kept,
        overlaps: true,
        path_overlaps: true,
        off_path_edges: true,
        paths: true,
        multi_edges: true,
    };
    /// A VG path has no per-step overlap; edges keep theirs.
    pub const VG: Self = Self {
        path_overlaps: false,
        ..Self::GFA
    };
    /// `vg view` of a VG file: ids as the VG writer chose them, no overlaps.
    pub const VG_VIEW: Self = Self {
        names: Names::Renumbered,
        overlaps: false,
        ..Self::VG
    };
    /// GBZ derives the graph from the paths in the GBWT.
    pub const GBZ: Self = Self {
        overlaps: false,
        path_overlaps: false,
        off_path_edges: false,
        multi_edges: false,
        ..Self::GFA
    };
    pub const ODGI: Self = Self {
        names: Names::Renumbered,
        overlaps: false,
        path_overlaps: false,
        ..Self::GFA
    };
    pub const FASTG: Self = Self {
        overlaps: false,
        path_overlaps: false,
        paths: false,
        ..Self::GFA
    };

    /// A VG file written by vg from `source`: vg keeps integer names as ids
    /// and renumbers other names in its own order.
    #[must_use]
    pub fn written_by_vg(source: &CoreGraphDTO) -> Self {
        let names = if has_numeric_names(source) {
            Names::Renumbered
        } else {
            Names::BySequence
        };
        Self::VG.with_names(names)
    }

    #[must_use]
    pub const fn with_names(self, names: Names) -> Self {
        Self { names, ..self }
    }
}

/// `(from, from_reverse, to, to_reverse, overlap)`, the smaller of an edge and
/// its mirror.
pub type LinkKey = (String, bool, String, bool, u32);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalPath {
    pub steps: Vec<(String, bool)>,
    /// Empty when every overlap is zero.
    pub overlaps: Vec<u32>,
}

/// A graph reduced to what can be compared across formats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalGraph {
    /// `(label, upper-case sequence)` → count.
    pub segments: BTreeMap<(String, String), u32>,
    /// Canonical link → multiplicity.
    pub links: BTreeMap<LinkKey, u32>,
    /// Path name → steps. A repeated name gets a `#dup<i>` suffix.
    pub paths: BTreeMap<String, CanonicalPath>,
}

/// Canonical form of the link `from -> to`.
#[must_use]
pub fn link(from: &str, from_rev: bool, to: &str, to_rev: bool, overlap: u32) -> LinkKey {
    let forward = (from.to_owned(), from_rev, to.to_owned(), to_rev, overlap);
    let mirror = (to.to_owned(), !to_rev, from.to_owned(), !from_rev, overlap);
    forward.min(mirror)
}

/// Whether `name` is a positive integer written without leading zeros.
fn positive_int(name: &str) -> Option<u64> {
    name.parse::<u64>()
        .ok()
        .filter(|&n| n > 0 && n.to_string() == name)
}

/// Whether every node is named by a distinct, positive, canonically written
/// integer: the names VG and ODGI keep as node ids, and the only ones
/// `odgi build` accepts.
#[must_use]
pub fn has_numeric_names(g: &CoreGraphDTO) -> bool {
    let mut seen = BTreeSet::new();
    (0..g.nodes().len())
        .all(|id| positive_int(&g.get_name_from_id(id)).is_some_and(|n| seen.insert(n)))
}

fn labels(g: &CoreGraphDTO, names: Names) -> Vec<String> {
    match names {
        Names::Kept => (0..g.nodes().len())
            .map(|id| g.get_name_from_id(id))
            .collect(),
        Names::Renumbered if has_numeric_names(g) => labels(g, Names::Kept),
        Names::Renumbered => (1..=g.nodes().len()).map(|id| id.to_string()).collect(),
        Names::BySequence => g.nodes().iter().map(|n| sequence(&n.sequence)).collect(),
    }
}

fn sequence(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_ascii_uppercase()
}

const fn is_rev(o: Orientation) -> bool {
    matches!(o, Orientation::Reverse)
}

/// Links traversed by a path, by node id, each in canonical orientation.
fn traversed(g: &CoreGraphDTO) -> BTreeSet<(NodeId, bool, NodeId, bool)> {
    g.paths()
        .iter()
        .flat_map(|p| p.steps.windows(2))
        .map(|w| {
            let (a, ar, b, br) = (
                w[0].node_id(),
                w[0].is_reverse(),
                w[1].node_id(),
                w[1].is_reverse(),
            );
            (a, ar, b, br).min((b, !br, a, !ar))
        })
        .collect()
}

/// Projects `g` onto what a format with fidelity `f` keeps.
#[must_use]
pub fn canonical(g: &CoreGraphDTO, f: Fidelity) -> CanonicalGraph {
    let label = labels(g, f.names);

    let mut segments = BTreeMap::new();
    for node in g.nodes().iter() {
        *segments
            .entry((label[node.id].clone(), sequence(&node.sequence)))
            .or_insert(0) += 1;
    }

    // Without off-path edges, only the links some path traverses survive.
    let on_path = (!f.off_path_edges).then(|| traversed(g));
    let mut links = BTreeMap::new();
    for e in g.edges() {
        let (a, ar, b, br) = (
            e.from_node,
            is_rev(e.from_orient),
            e.to_node,
            is_rev(e.to_orient),
        );
        if on_path
            .as_ref()
            .is_some_and(|kept| !kept.contains(&(a, ar, b, br).min((b, !br, a, !ar))))
        {
            continue;
        }
        let overlap = if f.overlaps { e.overlap } else { 0 };
        *links
            .entry(link(&label[a], ar, &label[b], br, overlap))
            .or_insert(0) += 1;
    }
    if !f.multi_edges {
        for m in links.values_mut() {
            *m = 1;
        }
    }

    let mut paths = BTreeMap::new();
    if f.paths {
        for p in g.paths() {
            let steps = p
                .steps
                .iter()
                .map(|s| (label[s.node_id()].clone(), s.is_reverse()))
                .collect();
            let overlaps = if f.path_overlaps && p.overlaps.iter().any(|&o| o != 0) {
                p.overlaps.clone()
            } else {
                Vec::new()
            };
            let name = String::from_utf8_lossy(&p.name).into_owned();
            let mut key = name.clone();
            let mut dup = 1;
            while paths.contains_key(&key) {
                key = format!("{name}#dup{dup}");
                dup += 1;
            }
            paths.insert(key, CanonicalPath { steps, overlaps });
        }
    }

    CanonicalGraph {
        segments,
        links,
        paths,
    }
}

/// Debug output cut to a readable length.
fn short(value: &impl Debug) -> String {
    const MAX: usize = 160;
    let text = format!("{value:?}");
    if text.chars().count() <= MAX {
        text
    } else {
        format!("{}…", text.chars().take(MAX).collect::<String>())
    }
}

/// Up to `limit` entries that differ between two maps.
fn map_diff<K: Ord + Debug, V: PartialEq + Debug>(
    expected: &BTreeMap<K, V>,
    actual: &BTreeMap<K, V>,
    limit: usize,
) -> Vec<String> {
    let missing = expected.iter().filter_map(|(k, v)| match actual.get(k) {
        None => Some(format!("  missing    {} => {}", short(k), short(v))),
        Some(a) if a != v => Some(format!(
            "  changed    {}: expected {}, got {}",
            short(k),
            short(v),
            short(a)
        )),
        Some(_) => None,
    });
    let unexpected = actual
        .iter()
        .filter(|(k, _)| !expected.contains_key(k))
        .map(|(k, v)| format!("  unexpected {} => {}", short(k), short(v)));
    missing.chain(unexpected).take(limit).collect()
}

/// The first step where two paths differ.
fn path_diff(name: &str, expected: &CanonicalPath, actual: &CanonicalPath) -> String {
    if expected.steps == actual.steps {
        return format!(
            "  path {name}: same steps, overlaps {} vs {}",
            short(&expected.overlaps),
            short(&actual.overlaps)
        );
    }
    let at = expected
        .steps
        .iter()
        .zip(&actual.steps)
        .position(|(e, a)| e != a)
        .unwrap_or_else(|| expected.steps.len().min(actual.steps.len()));
    let window = |p: &CanonicalPath| short(&p.steps.get(at..(at + 3).min(p.steps.len())));
    format!(
        "  path {name}: {} vs {} steps, first difference at step {at}: expected {}, got {}",
        expected.steps.len(),
        actual.steps.len(),
        window(expected),
        window(actual),
    )
}

/// Panics with a readable diff when the graphs differ.
pub fn assert_same(expected: &CanonicalGraph, actual: &CanonicalGraph, context: &str) {
    if expected == actual {
        return;
    }
    let mut report = vec![format!(
        "graphs differ: {context}\n  segments {} vs {}, links {} vs {}, paths {} vs {}",
        expected.segments.len(),
        actual.segments.len(),
        expected.links.len(),
        actual.links.len(),
        expected.paths.len(),
        actual.paths.len(),
    )];
    for (what, diff) in [
        (
            "segments",
            map_diff(&expected.segments, &actual.segments, 5),
        ),
        ("links", map_diff(&expected.links, &actual.links, 5)),
    ] {
        if !diff.is_empty() {
            report.push(format!("{what}:"));
            report.extend(diff);
        }
    }
    let path_names: BTreeSet<&String> = expected.paths.keys().chain(actual.paths.keys()).collect();
    let paths: Vec<String> = path_names
        .into_iter()
        .filter_map(
            |name| match (expected.paths.get(name), actual.paths.get(name)) {
                (Some(e), Some(a)) if e != a => Some(path_diff(name, e, a)),
                (Some(_), None) => Some(format!("  missing path {name}")),
                (None, Some(_)) => Some(format!("  unexpected path {name}")),
                _ => None,
            },
        )
        .take(5)
        .collect();
    if !paths.is_empty() {
        report.push("paths:".to_owned());
        report.extend(paths);
    }
    panic!("{}", report.join("\n"));
}

/// Asserts that `actual` keeps everything of `expected` that fidelity `f` can
/// represent.
///
/// Off-path links are dropped from `expected` only: a format that cannot
/// store them must not invent them either.
pub fn assert_equivalent(
    expected: &CoreGraphDTO,
    actual: &CoreGraphDTO,
    f: Fidelity,
    context: &str,
) {
    let all_links = Fidelity {
        off_path_edges: true,
        ..f
    };
    assert_same(
        &canonical(expected, f),
        &canonical(actual, all_links),
        context,
    );
}

/// A file under `tests/test_files/`.
#[must_use]
pub fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/test_files")
        .join(rel)
}

/// The file name without its extension, for messages.
#[must_use]
pub fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Parses a file, panicking with its path on failure.
#[must_use]
pub fn load(path: &Path, format: GraphFormat) -> CoreGraphDTO {
    let text = path.to_str().expect("UTF-8 path");
    CoreGraphDTO::load_from_file(text, format)
        .unwrap_or_else(|e| panic!("cannot parse {} as {format}: {e}", path.display()))
}

/// Serializes `graph` as `format` in memory.
#[must_use]
pub fn to_bytes(graph: &CoreGraphDTO, format: GraphFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    graph
        .save(&mut bytes, format)
        .unwrap_or_else(|e| panic!("cannot write {format}: {e}"));
    bytes
}

/// Parses `bytes` as `format`.
#[must_use]
pub fn from_bytes(bytes: Vec<u8>, format: GraphFormat) -> CoreGraphDTO {
    CoreGraphDTO::load(&mut Cursor::new(bytes), format)
        .unwrap_or_else(|e| panic!("cannot read {format}: {e}"))
}

/// Writes `graph` as `format` and parses it back, in memory.
#[must_use]
pub fn round_trip(graph: &CoreGraphDTO, format: GraphFormat) -> CoreGraphDTO {
    from_bytes(to_bytes(graph, format), format)
}
