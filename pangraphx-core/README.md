# pangraphx-core

The library behind [PanGraphX](https://github.com/samuelburger27/PanGraphX): a fast,
parallel Rust crate for reading, writing and transforming pangenome graphs.

- Parse and serialize **every** supported format through one API — GFA 1.0/1.1, VG (protobuf), GBZ, ODGI (`.og`) and FASTG
- Two graph types: the transfer object `CoreGraphDTO` (what parsers return and serializers take) and the mutable, index-accelerated `CoreGraph`
- Graph statistics (N50, connected components, degree and path-length distributions)
- de Bruijn graphs (`DeBruijn`, colored `ColoredDBG`) for k = 1–63, optionally compacted into unitigs

Building requires `protoc` (the VG protobuf schema is compiled at build time).

## Usage

```toml
[dependencies]
pangraphx-core = "0.1"
```

```rust,no_run
use pangraphx_core::{CoreGraph, CoreGraphDTO, DeBruijn, GraphFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Convert GFA to GBZ
    let graph = CoreGraphDTO::load_from_file("graph.gfa", GraphFormat::GFA)?;
    graph.save_to_file("graph.gbz", GraphFormat::GBZ)?;

    // Statistics
    let stats = CoreGraph::new(graph.clone()).compute_stats();
    println!("{} nodes, N50 {} bp", stats.node_count, stats.node_len.n50);

    // de Bruijn graph with k = 31, compacted into unitigs
    let dbg = DeBruijn::from_directed_graph(graph, 31)?;
    dbg.into_compacted_dto()?.save_to_file("unitigs.gfa", GraphFormat::GFA)?;
    Ok(())
}
```

## Supported formats

Every format is both parsed and serialized. Format selection goes through
[`GraphFormat`], which also maps extensions (`from_extension`) and returns the
codec for either direction (`get_parser` / `get_serializer`).

| Format | Extension | Read | Write | Notes |
|---|---|---|---|---|
| GFA 1.0 / 1.1 | `.gfa` | ✓ | ✓ | `W` lines become paths with vg-style names and are written as `P` lines; `C`/`J` lines are ignored |
| VG (protobuf) | `.vg` | ✓ | ✓ | Per-step path overlaps are dropped |
| GBZ | `.gbz` | ✓ | ✓ | Keeps only what its paths imply (no overlaps, off-path or repeated edges); nodes are not chopped to 1024 bp |
| ODGI | `.og` | ✓ | ✓ | Native reader/writer (no `odgi` dependency); overlaps are dropped |
| FASTG | `.fastg` | ✓ | ✓ | SPAdes dialect; paths and overlaps are dropped |

VG and ODGI preserve node ids only when every node name is a distinct, positive,
canonically written integer; otherwise nodes are renumbered `1..=n`.

## Graph types

### `CoreGraphDTO`

The plain-data transfer object. Parsers produce it and serializers consume it,
so its fields (`nodes`, `edges`, `paths`, `node_name_map`) are public for direct
indexing. It also provides convenience helpers:

- `load_from_file` / `save_to_file` for paths, and `load` / `save` for streams
- `get_name_from_id` / `get_node_name` to resolve original node names
- `dangling_reference()` to validate that edges and path steps refer to existing nodes

### `CoreGraph`

The mutable form, built with `CoreGraph::new(dto)` and convertible back with
`CoreGraphDTO::from(graph)`. On top of the DTO's data it maintains an
`adjacency_list` (node id → edge indices) and a `path_map` (name → path) for
fast lookup and editing.

```rust
use pangraphx_core::{CoreGraph, CoreGraphDTO, Edge, Orientation, Path, Step};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = CoreGraph::new(CoreGraphDTO::default());
    let a = graph.add_node(b"ACGT".to_vec(), None);
    let b = graph.add_node(b"TGCA".to_vec(), None);

    graph.add_edge(Edge {
        from_node: a, from_orient: Orientation::Forward,
        to_node: b,   to_orient: Orientation::Forward,
        overlap: 0,
    })?;

    graph.add_path(Path {
        name: b"path1".to_vec(),
        steps: vec![Step::new(a, Orientation::Forward)?],
        overlaps: vec![],
    })?;

    assert!(graph.has_edge(a, b));
    for edge in graph.get_outgoing_edges(a)? {
        // ...
    }
    Ok(())
}
```

## Parsing and serialization

Everything reads from a `Read + Seek` stream and writes to a `Write` stream, so
large graphs are streamed rather than buffered whole.

```rust,no_run
use std::fs::File;
use pangraphx_core::{CoreGraphDTO, GraphFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // From/to a file by path
    let graph = CoreGraphDTO::load_from_file("graph.gfa", GraphFormat::GFA)?;
    graph.save_to_file("graph.vg", GraphFormat::VG)?;

    // From/to arbitrary streams
    let mut input = File::open("graph.gfa")?;
    let mut output = File::create("graph.gbz")?;
    let graph = CoreGraphDTO::load(&mut input, GraphFormat::GFA)?;
    graph.save(&mut output, GraphFormat::GBZ)?;

    // Or pick the codec directly, without the DTO helpers
    let parser = GraphFormat::GFA.get_parser();
    let mut input = File::open("graph.gfa")?;
    let _graph = parser.parse(&mut input)?;
    Ok(())
}
```

Codecs implement the [`GraphParser`] and [`GraphSerializer`] traits, so custom
formats can plug into the same dispatch machinery. Resolve a format from a
filename extension string with `GraphFormat::from_extension("gfa")`.

## Performance

PanGraphX is built for large graphs (e.g. HPRC chr22): streaming I/O, an
in-house two-pass GFA parser, a parallel chunked GFA writer, sharded concurrent
k-mer sets, and `rayon` parallelism throughout. End-to-end conversions measured
via the CLI on HPRC v1.1 chr22 (659 MB GFA, 1.28 M nodes):

| Conversion | vs. | PanGraphX | reference | Speedup |
|---|---|---:|---:|---:|
| GFA → VG | `vg convert` | **1.8 s** / 0.6 GiB | 269.8 s / 17.3 GiB | **150×** (29× less memory) |
| VG → GFA | `vg convert` | **7.2 s** / 2.0 GiB | 38.2 s / 4.2 GiB | **5.3×** |
| GFA → OG | `odgi build` | **2.5 s** / 1.4 GiB | 7.2 s / 1.6 GiB | **2.9×** |
| OG → GFA | `odgi view` | **2.6 s** / 1.3 GiB | 6.7 s / 0.9 GiB | **2.6×** |
| GFA → GBZ | `vg gbwt` | **33.4 s** / 1.3 GiB | 36.3 s / 1.8 GiB | **1.1×** |

Wall-clock time (mean of 3 runs) / peak RSS against vg v1.73.0 and odgi
v0.9.4, all threads. Full results and reproduction: [`benchmarks/README.md`](../benchmarks/README.md).

## Statistics

[`CoreGraph::compute_stats`] returns a [`GraphStats`] summary:

- `node_count`, `edge_count`, `path_count`
- `node_len`: min / max / mean / total / **N50** of node sequence lengths
- `component_count` and `largest_component` (weakly connected)
- `isolated_nodes`, plus in- and out-degree summaries and a total-degree histogram
- `path_len`: path lengths in base pairs (overlap-adjusted) and in steps

## de Bruijn graphs

Build a node-centric de Bruijn graph of order `k` (`1..=63`; k-mers are 2-bit
packed into `u64` for k ≤ 31, `u128` for k ≤ 63):

- [`DeBruijn::from_directed_graph`] — k-mers of the sequences spelled by the graph's paths
- [`DeBruijn::from_directed_graph_full_topology`] — every walk of the bidirected graph, ignoring the paths
- [`ColoredDBG::from_directed_graph`] — like the first, but keeps each input path as a walk through the k-mers

Nodes are distinct canonical k-mers, edges distinct canonical (k+1)-mers; node
ids follow the sorted 2-bit codes, so output is deterministic regardless of
thread count. Convert back to a `CoreGraphDTO` with `TryFrom`/`try_from`, or
merge non-branching chains into unitigs with `into_compacted_dto()`.

## License

MIT
