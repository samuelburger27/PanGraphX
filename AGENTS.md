# PanGraphX 
## Project Guidelines

PanGraphX is a Rust workspace consisting of a core library (`pangraphx-core`) and a CLI tool (`pangraphx-cli`) for converting, inspecting, and manipulating Pangenome Graph formats.

- **Key Crates**: `anyhow` (CLI error handling), `thiserror` (Library error definitions), `clap` (CLI argument parsing), `log` (logging).

## Rules
Every change implement in a separate branch describing the request,  commit your work with short descriptive commit messages, **do not push your changes**

## Code Style

### Error Handling
- **Library (`pangraphx-core`)**: 
  - Define errors using `thiserror` in `pangraphx-core/src/error.rs`.
  - Public APIs must return `PanResult<T>` (alias for `Result<T, PanGraphXError>`).
  - **Do not panic**: Avoid `unwrap()` and `expect()` in library code; check constraints and return `PanGraphXError`.
  - Convert external errors (e.g., `std::io::Error`, Parser errors) into specific variants of `PanGraphXError`.
- **CLI (`pangraphx-cli`)**: 
  - Use `anyhow::Result` in `main.rs` and command handlers to catch errors at the top level.
  - Report usage errors user-friendly via `clap`.

### Logging
- Use standard `log` macros (`debug!`, `info!`, `warn!`, `error!`).
- Do not use `println!` for logging unless it is direct CLI output (e.g., `info` command result).
- Enable logging via `RUST_LOG` environment variable (CLI uses `env_logger`).

## Architecture

### Component Structure
- **`pangraphx-core`**: The main library crate.
  - `core/`: Internal graph representation (`CoreGraph`, `CoreGraphDTO`).
  - `formats/`: Format-specific logic (GFA, GBZ, VG, FastG, ODGI).
    - Each format has a codec struct (e.g., `GBZCodec`, `GFACodec`) implementing generic traits.
    - `gfa_parser.rs`: in-house streaming two-pass GFA parser (no external GFA crate); `odgi_format.rs`: native reader/writer for odgi's binary `.og` layout (format spec in `docs/odgi_format.md`, `utils/dump_odgi.py` dumps a file; no `odgi` dependency); `id_map.rs`: shared external-integer-id → `NodeId` lookup (dense table or hash map) used by VG, GBZ and numeric GFA names.
    - Serializers check `CoreGraphDTO::dangling_reference()` once, then index nodes directly.
  - `de_bruijn_conversion/`: Algorithms for de Bruijn graph construction and k-mer processing.
    - `word.rs` (2-bit `u64`/`u128` words, `RollingWindow`), `extraction.rs` (streaming path / bidirected topology walkers), `kmer_set.rs` (sharded concurrent set), `compaction.rs` (unitigs). `DeBruijn` stores sorted k-mer and canonical (k+1)-mer tables; node id = rank. Keep per-k-mer state to a few bytes and never materialize per-path k-mer vectors.
  - `traits/`: Defines `GraphParser` and `GraphSerializer`.
- **`pangraphx-cli`**: The command-line interface.
  - `cli/args_parser.rs`: `clap` definitions for subcommands (`Convert`, `DBG`, `Info`, `Formats`).
  - `src/main.rs`: Entry point dispatching commands to handlers in `src/`.

### Key Abstractions
- **Format Handling**: Centralized in `GraphFormat` enum (`pangraphx-core/src/lib.rs`).
- **Traits**:
  - `GraphParser<R>`: Parse from a reader.
  - `GraphSerializer<W>`: Serialize to a writer.

## Build and Test

- **Build**:
  ```bash
  cargo build --workspace
  ```
  Benchmarks use the `dist` profile (fat LTO, `panic = "abort"`): `cargo build --profile dist` → `target/dist/pangraphx-cli`. Optional `mimalloc` CLI feature trades peak RSS for speed.
- **Test**:
  - Run all tests:
    ```bash
    cargo test --workspace
    ```
  - Run specific format tests:
    ```bash
    cargo test --package pangraphx-core -- formats::gfa_format
    ```

## Project Conventions

### Integration Points
- **New Format Support**:
  1. Create a module in `pangraphx-core/src/formats/`.
  2. Implement `GraphParser` and/or `GraphSerializer`.
  3. Register in `GraphFormat` enum (`pangraphx-core/src/lib.rs`).
  4. Export via `pangraphx-core/src/formats/mod.rs`.

- **External Tools**:
  - `utils/generate_gfa.py`: Helper script to generate GFA files for testing.
  - `utils/download_gfa.sh`: Fetches the large benchmark graphs into `benchmarks/data/`.
  - `benchmarks/`: `compare_tools.sh` (time + peak RSS vs vg/odgi via hyperfine), `dbg.sh` (memory-guarded DBG sweep), `setup_ubuntu.sh` (fresh Ubuntu: deps, pinned vg/odgi, dist build, graph downloads); shared helpers in `lib.sh`/`ops.sh`. See `benchmarks/README.md`. Never run DBG benchmarks in parallel — memory grows with the number of distinct k-mers (≈ node sequence length), and uncompacted output dominates.
  - `utils/generate_test_files.sh`: Writes the committed `tests/test_files/{vg,gbz,og}/` files and `MANIFEST.tsv` with `vg`/`odgi` from the GFA fixtures (never with PanGraphX).
  - ODGI's `.og` layout is odgi-internal and unversioned: after changing `odgi_format.rs` (or the VG/GBZ codecs), run `cargo test --release -p pangraphx-core --test cross_validation` with `vg`/`odgi` on `PATH` or in `PANGRAPHX_VG`/`PANGRAPHX_ODGI`. It checks our files with `vg validate`/`vg view`/`vg convert -f`/`odgi validate`/`odgi view` and parses files written by `vg convert`/`vg gbwt`/`odgi build`; without a tool it prints `SKIPPED`, and `PANGRAPHX_REQUIRE_EXTERNAL=1` turns a skip into a failure (CI job `cross-validate`). Pinned tool versions live in `utils/install_external_tools.sh`.
  - Test comparisons go through `pangraphx-core/tests/common` (`canonical` + `Fidelity` per format), not `CoreGraphDTO::isomorphic`. The vg/odgi-written files in `tests/test_files/{vg,gbz,og}/` are checked by `tests/test_reference_files.rs` without the tools; writer tests round-trip in memory.
  - The project interacts with standard bioinformatics formats (GFA v1.0, GFA v1.1 locally supported extensions, GBZ, VG, ODGI).

## Core Data Types

### Identity and Sequences
- **`NodeId` (type alias `usize`)**: Unique identifier for a node/segment. Always corresponds to the index in the `Nodes` vector.
- **`Sequence`** (type alias `Vec<u8>`): DNA sequence data. Stored as UTF-8 bytes.
- **`NodeName`** and **`PathName`** (type alias `Vec<u8>`): Identifiers for nodes and paths, stored as bytes.
- **`Orientation`** (enum): Represents traversal direction—`Forward` (+) or `Reverse` (-).

### Node
[Node](pangraphx-core/src/core/core_types.rs#L43-L46) represents a segment in the graph with a DNA sequence and unique ID. Used as the fundamental building block of the pangenome graph.
```rust
pub struct Node {
    pub sequence: Sequence,
    pub id: NodeId,
}
```

### Edge
[Edge](pangraphx-core/src/core/core_types.rs#L49-L55) connects two nodes with specific orientations and overlap. Edges are directed and include overlap metadata for variant representation.
```rust
pub struct Edge {
    pub from_node: NodeId,
    pub from_orient: Orientation,
    pub to_node: NodeId,
    pub to_orient: Orientation,
    pub overlap: u32,
}
```

### Path
[Path](pangraphx-core/src/core/core_types.rs#L67-L71) represents a named, ordered traversal through nodes. Paths are essential for representing genome sequences or haplotypes in the pangenome.
```rust
pub struct Path {
    pub name: PathName,
    pub steps: Vec<Step>,     // Sequence of node traversals
    pub overlaps: Vec<u32>,   // Overlaps between consecutive steps
}
```
**Step** (within a Path): A node traversed in an orientation, packed into a `u32` handle (`node_id << 1 | is_reverse`). Build with `Step::new(node_id, orientation)` (fails with `NodeIdTooLarge` above `Step::MAX_NODE_ID` = 2^31 − 1) and read with `node_id()`, `orientation()`, `is_reverse()`.

### Collections
- **[`Nodes`](pangraphx-core/src/core/core_types.rs#L75-L76)**: A wrapper collection enforcing the invariant that node IDs correspond to their vector indices. Always use this instead of raw `Vec<Node>`.
- **[`CoreGraphDTO`](pangraphx-core/src/core/graph_dto.rs)**: Immutable transfer object containing `nodes`, `edges`, `paths`, and optional `node_name_map`.
- **[`CoreGraph`](pangraphx-core/src/core/graph.rs)**: Mutable graph type with an `adjacency_list` HashMap for efficient edge lookup. Use for graph manipulation.

### Key Invariants
1. **Node Index Consistency**: In `Nodes`, `node.id` must always equal its index in the vector. The `from_node_vec()` constructor enforces this.
2. **NodeId Validity**: All `NodeId` references in edges and paths must point to valid nodes. Constructors validate this.
3. **Adjacency List Sync**: In `CoreGraph`, the adjacency list is built at construction time. Keep it in sync when adding/removing edges.

## Security

- **Input Validation**: Validate file formats strictly against specifications during parsing.
- **Safe Rust**: Prefer safe Rust. Avoid `unsafe` blocks unless strictly necessary for performance in hot loops, and document safety contracts clearly.
