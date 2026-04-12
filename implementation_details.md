# Implementation Details

## Technology Stack

PanGraphX is implemented in Rust (edition 2024) and organized as a Cargo workspace consisting of two crates:

- **`pangraphx-core`** – a reusable library that encapsulates the internal graph representation, format codecs and de Bruijn graph algorithms.
- **`pangraphx-cli`** – a thin command line front‑end that exposes the library functionality as subcommands.

The core crate depends on several domain‑specific libraries:

- **`gfa`** for parsing and emitting GFA v1.x graph files.
- **`gbwt`** and **`odgi-ffi`** for working with GBZ/GBWT and ODGI‑style pangenome graphs.
- **`bio`** and **`simple-sds-sbwt`** for sequence processing and succinct data structures.
- **`rayon`** for data‑parallel computations on CPUs.
- **`thiserror`** and **`log`** for structured error handling and logging.

The CLI crate depends on:

- **`clap`** for declarative argument parsing.
- **`anyhow`** for ergonomic error propagation in the CLI layer.
- **`env_logger`** and **`log`** for runtime‑configurable logging.
- **`pangraphx-core`** as its only domain‑specific dependency.

This separation keeps the core logic testable and reusable while allowing the CLI to evolve independently.

## Parsing and Serialization

All external formats are mapped to a **common in‑memory representation** described by the `CoreGraphDTO` type. This data transfer object groups:

- `nodes: Nodes` – sequences with stable integer identifiers.
- `edges: Vec<Edge>` – directed links between nodes with orientation and overlap.
- `paths: Vec<Path>` – named walks through the graph.
- `node_name_map: Option<HashMap<NodeId, NodeName>>` – an optional mapping that preserves original segment names.

Two traits define the parsing and serialization interfaces:

```rust
pub trait GraphParser<R: Read + Seek> {
    fn parse(&self, reader: &mut R) -> PanResult<CoreGraphDTO>;
}

pub trait GraphSerializer {
    fn serialize(&self, graph: &CoreGraphDTO, writer: &mut dyn Write) -> PanResult<()>;
}
```

For each supported format (GFA, GBZ, VG, FASTG, …) there is a codec type that implements these traits. The `GraphFormat` enum acts as a factory, returning the appropriate parser/serializer based on the requested format or file extension.

### Example: GFA Codec

The GFA implementation (`GFACodec`) illustrates the general pattern.

1. **Parsing phase**
   - The input stream is wrapped in a `BufReader` and fully read into memory as a vector of lines.
   - The `gfa` crate’s `GFAParser` parses these lines into its own `GFA` structure.
   - Nodes are created sequentially to preserve ordering. For every GFA segment the sequence is copied into `Nodes`, and a mapping from index to original name is recorded.
   - A reverse lookup map (`HashMap<&[u8], usize>`) is built from segment name to internal `NodeId`.
   - Edges are constructed in parallel using `into_par_iter()` over `gfa.links`. Each link is converted into an `Edge` using the reverse map and a helper that turns CIGAR strings into overlap lengths. Errors in mapping are reported as `PanGraphXError::Parse` values.
   - Paths are also constructed in parallel from `gfa.paths`. For each path, the segment names are resolved into `Step` objects (node id + orientation), and overlaps are converted from optional CIGARs to `u32` values.
   - The resulting `CoreGraphDTO` stores nodes, edges, paths and the `node_name_map` so that original segment names can be reconstructed during serialization.

2. **Serialization phase**
   - A GFA header line is written sequentially.
   - Node, edge and path lines are **formatted in parallel** using `par_iter()` over `graph.nodes`, `graph.edges` and `graph.paths` respectively. Each worker thread converts its item into an in‑memory `String`.
   - The resulting vectors of lines are then written back to the output stream sequentially. This design maximizes CPU‑bound work in parallel while keeping I/O ordered and simple.

Other codecs (GBZ, VG, FASTG) follow the same architecture: they are thin adapters between the external format and `CoreGraphDTO`, isolating all format‑specific logic from the rest of the system.

## De Bruijn Graph Construction Algorithm

PanGraphX can derive (colored) de Bruijn graphs from an arbitrary directed genome graph represented by `CoreGraphDTO`. The implementation is located in the `de_bruijn_conversion` module and is built around three concepts: k‑mer encoding, oriented k‑mers, and graph construction.

### K‑mer encoding

A k‑mer is encoded into a compact `u128` value using 2‑bit encoding:

- `A/a/N → 0`, `C/c → 1`, `G/g → 2`, `T/t/other → 3`.

The `Kmer` struct stores this code together with `k`, the k‑mer length. Helper functions support:

- Rolling updates (`roll_kmer`) that slide a window one base to the right.
- Reverse‑complement computation by reversing the 2‑bit code and complementing bases.
- A **canonical form** that picks the lexicographically smaller of the forward and reverse‑complement sequences. Canonicalization ensures that each biological k‑mer corresponds to a unique graph vertex, independent of strand.

Decoding reconstructs the ASCII sequence from the 2‑bit code when needed, for example during debugging or serialization.

### Oriented k‑mers

To retain orientation information, the code introduces

```rust
pub struct OrientedKmer {
    pub kmer: Kmer,
    pub direction: Orientation,
}
```

An `OrientedKmer` wraps a canonical `Kmer` together with the direction in which it was observed (forward or reverse). This allows the algorithm to model edge directions and path orientation while still deduplicating k‑mers by canonical sequence.

### Standard de Bruijn graph

The main construction function

```rust
impl DeBruijn {
    pub fn from_directed_graph(graph: CoreGraphDTO, k: usize) -> Self { … }
}
```

performs the following steps:

1. Wrap the DTO in a `CoreGraph` helper that can efficiently traverse adjacency lists and extract k‑mers.
2. Call `extract_kmers_paths(k)` to obtain, for each haplotype path, a sequence of `OrientedKmer` values representing all k‑mers along that path.
3. Process these path–k‑mer sequences in parallel using Rayon’s `into_par_iter()` combined with a `fold`/`reduce` pattern:
   - Each worker thread maintains local `HashSet<Kmer>` and `HashSet<DbgEdge>` collections.
   - For every window of size two (`kmers.windows(2)`), it inserts the two endpoint k‑mers into the local k‑mer set and a `DbgEdge { from, to }` into the edge set.
   - After processing its share of paths, the thread returns its local sets.
4. The `reduce` step merges all partial sets into a global set of vertices and edges by unioning the hash sets.

The resulting `DeBruijn` struct contains the deduplicated set of k‑mers, the set of directed edges between oriented k‑mers, and the chosen k‑mer length. Construction is linear in the number of k‑mers (up to hash table overhead) and parallel over paths.

A variant

```rust
pub fn from_directed_graph_full_topography(graph: CoreGraphDTO, k: usize) -> Self
```

calls `extract_kmers_from_full_topology(k)` instead. Rather than following only explicit haplotype paths, it considers all topological walks induced by adjacency in the original graph. This captures edges that might not appear along any named path, at the cost of potentially much larger intermediate k‑mer sets.

### Colored de Bruijn graph

The `ColoredDBG` type extends `DeBruijn` with per‑path color information:

- Each input path is converted into a `ColorPath`, which stores the sequence of `OrientedKmer` values along that path.
- Construction uses the same parallel `fold`/`reduce` pattern as the uncolored case, but threads additionally accumulate `ColorPath` objects.
- After reduction, `ColoredDBG` holds a `DeBruijn` instance plus a vector of color paths that encode which original path(s) each k‑mer belongs to.

Both `DeBruijn` and `ColoredDBG` implement `From<T> for CoreGraphDTO`. The conversion back to the generic graph representation works as follows:

1. Enumerate all k‑mers and assign each a `NodeId`. Build a `HashMap<Kmer, Node>` that maps every k‑mer to the corresponding `Node` with `sequence = kmer.to_bytes()`.
2. Translate each de Bruijn edge into an `Edge` between the corresponding node ids, preserving orientation. The overlap field is set to `k - 1`, which is the number of shared bases between consecutive k‑mers.
3. For colored graphs, each `ColorPath` is turned into a `Path` whose steps reference the de Bruijn nodes. Paths are named deterministically (e.g. `C0`, `C1`, …).
4. Nodes are materialized in ID order using `Nodes::from_seq`, yielding a consistent `CoreGraphDTO` with nodes, edges and (optionally) paths.

This design allows the de Bruijn graph to participate in the same conversion and serialization pipeline as any other `CoreGraphDTO` instance.

## Concurrency

Concurrency in PanGraphX is expressed entirely through **Rayon’s data‑parallel iterators**. The guiding principle is to parallelize pure, CPU‑bound computations while keeping I/O and shared state simple.

Key examples include:

- **GFA parsing and serialization**: construction of edges and paths from the parsed GFA structure, as well as formatting of nodes, edges and paths into strings, is parallelized using `into_par_iter()` and `par_iter()`. Shared maps (such as the segment‑name lookup table) are read‑only and therefore thread‑safe.
- **CoreGraphDTO utilities**: the `isomorphic` method compares two graphs by checking, in parallel, that every edge in one graph has a corresponding edge in the other when nodes are matched by sequence.
- **Node collections**: the `Nodes` wrapper exposes `par_iter()` and `par_iter_mut()`, enabling callers to process or transform all nodes in parallel when appropriate.
- **De Bruijn construction**: both the standard and colored de Bruijn builders process path‑k‑mer sequences in parallel, aggregating per‑thread hash sets and reducing them into global sets.

The global Rayon thread pool is used with its default configuration, which typically spawns one worker per logical CPU. At the CLI level, the `convert` and De Bruijn subcommands accept a `--threads` parameter that is intended to control the degree of parallelism. In the current code snapshot this parameter is parsed but not yet wired into a custom Rayon thread pool; thus, experiments described in the thesis used Rayon’s default behaviour unless otherwise stated.

I/O operations (reading input files and writing outputs) are performed sequentially on a single thread. Only CPU‑intensive transformations (parsing, k‑mer extraction, de Bruijn edge construction, and string formatting) are parallelized. This avoids subtle concurrency bugs around file handles while still exploiting multi‑core processors effectively.

## The Command Line Interface

The `pangraphx-cli` crate turns the library into a user‑friendly command line tool. Argument parsing is implemented with `clap` derive macros:

```rust
#[derive(Parser, Debug)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Convert(ConvertArgs),
    Ddb(DeBruijnArgs),
    Info(InfoArgs),
    Validate(ValidateArgs),
    Formats,
}
```

Each variant corresponds to a logical operation:

- **`convert`** – format conversion between any two supported graph types. `ConvertArgs` include input/output paths, optional explicit `from`/`to` formats, and a `threads` parameter. The implementation infers formats from either these flags or file extensions, loads a `CoreGraphDTO` via `CoreGraphDTO::load_from_file`, and writes the result with `save_to_file`.
- **`ddb`** – construction of (colored) de Bruijn graphs. `DeBruijnArgs` add `kmer_size`, `colored` and `full_topology` switches on top of the usual input/output fields. The handler selects between the three construction modes (`DeBruijn::from_directed_graph`, `from_directed_graph_full_topography`, `ColoredDBG::from_directed_graph`) and finally serializes the resulting `CoreGraphDTO`.
- **`info`** and **`validate`** – placeholders for future features that will inspect and validate graph files. At the time of writing they are stubbed with `todo!()` but their argument structures and logging hooks are already in place.
- **`formats`** – lists all supported formats by iterating over the `GraphFormat` enum and printing each variant together with its canonical file extension.

The `main` function wires everything together:

1. Initialize logging via `env_logger::init()`, so the user can enable debug output with the `RUST_LOG` environment variable.
2. Parse the command line into a `Cli` value using `Cli::parse()`.
3. Dispatch to the appropriate handler based on the selected subcommand.
4. Propagate errors using `anyhow::Result`, which allows rich error contexts from the core library to be surfaced to the user.

Overall, the CLI is intentionally thin: it performs argument validation, logging and user interaction, while delegating all heavy lifting—including parsing, format conversion, and de Bruijn graph construction—to the `pangraphx-core` crate.