# PanGraphX

A fast and efficient Rust library and command-line tool for working with pangenome graphs. PanGraphX provides seamless conversion between multiple genome graph formats and includes tools for de Bruijn graph construction and genome graph analysis.

## Features

- **Format Conversion**: Convert between GFA, GBZ, VG, and FASTG formats
- **Graph Inspection**: Query basic information about graph files
- **Graph Statistics**: Report sequence-length metrics (incl. N50), connected components, degree distribution, and path lengths
- **Format Validation**: Validate graph structure and format compliance
- **De Bruijn Graphs**: Construct de Bruijn graphs from existing genome graphs with configurable k-mer sizes
- **Colored De Bruijn Graphs**: Generate colored de Bruijn graphs for multi-sample analysis
- **Library & CLI**: Use as a Rust library or standalone command-line tool

## Supported Formats

- **GFA** (Graphical Fragment Assembly) v1.0, v1.1 with local extensions
- **GBZ** (Graph Bioinformatics Zipped) - compressed graph format
- **VG** (Variation Graph) - protobuf-based variation graph format
- **FASTG** (FASTA Gapped) - sequence format with assembly graph metadata
- **ODGI** (Optimized Dynamic Genome/Graph Implementation) - requires `odgi` feature flag, Linux only

## Quick Start

### Installation

#### From Source

```bash
git clone https://github.com/samuelburger27/PanGraphX.git
cd PanGraphX
cargo build --release
```

The compiled binary will be available at `target/release/pangraphx-cli`.

#### With ODGI Support (Linux Only)

ODGI support requires native C++ libraries that only build on Linux. Enable it with the `odgi` feature flag:

```bash
cargo build --workspace --features pangraphx-core/odgi --release
```

### CLI Usage

#### List Supported Formats

```bash
pangraphx-cli formats
```

#### Convert Between Formats

Convert a GFA file to GBZ format with automatic format detection:

```bash
pangraphx-cli convert -i graph.gfa -o graph.gbz
```

Explicitly specify input and output formats:

```bash
pangraphx-cli convert -i graph.gfa -o graph.gbz --from gfa --to gbz
```

#### Inspect Graph Files

Display basic information about a graph:

```bash
pangraphx-cli info graph.gfa
```

#### Compute Graph Statistics

Print detailed statistics about a graph — sequence-length distribution (total,
min/avg/max, N50), the number of weakly connected components, the in/out degree
distribution, and the path-length distribution:

```bash
pangraphx-cli stats graph.gfa
```

Override the input format when it cannot be inferred from the file extension:

```bash
pangraphx-cli stats graph.dat --format gfa
```

Example output:

```text
Graph Statistics:
--------------------------------------------------
 Sequence
   Nodes:             169
   Total length:      5,386 bp
   Node length:       min 10 / avg 31.9 / max 32 bp
   N50:               32 bp
 Topology
   Edges:             168
   Components:        1   (largest 169 nodes)
   Isolated nodes:    0
   In-degree:         min 0 / avg 0.99 / max 1
   Out-degree:        min 0 / avg 0.99 / max 1
   Degree histogram:  1:2  2:167
 Paths
   Paths:             1
   Path length:       min 5,386 / avg 5386.0 / max 5,386 bp   (total 5,386 bp)
   Path steps:        min 169 / avg 169.0 / max 169
```

#### Validate Graph Files

Check graph structure and format compliance:

```bash
pangraphx-cli validate graph.gfa
```

#### De Bruijn Graph Construction

Convert a variation graph to a de Bruijn graph with k-mer size 31:

```bash
pangraphx-cli dbg -i graph.vg -k 31 -o dbg_graph.gfa
```

Generate a colored de Bruijn graph (for multi-sample analysis):

```bash
pangraphx-cli dbg -i graph.vg -k 31 -c -o colored_dbg.gfa
```

Use full topology (all topological walks instead of just haplotype paths):

```bash
pangraphx-cli dbg -i graph.vg -k 31 --full-topology -o full_dbg.gfa
```

### Library Usage

Add PanGraphX to your `Cargo.toml`:

```toml
[dependencies]
pangraphx-core = { path = "./pangraphx-core" }
```

#### Parse a Graph

```rust
use std::fs::File;
use pangraphx_core::{GraphFormat, traits::GraphParser};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::open("graph.gfa")?;
    let parser = GraphFormat::GFA.get_parser();
    let graph = parser.parse(&mut file)?;
    
    println!("Graph loaded successfully");
    Ok(())
}
```

#### Convert Between Formats Code

```rust
use std::fs::File;
use pangraphx_core::{GraphFormat, traits::{GraphParser, GraphSerializer}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse GFA
    let mut input = File::open("graph.gfa")?;
    let parser = GraphFormat::GFA.get_parser();
    let graph = parser.parse(&mut input)?;
    
    // Serialize to GBZ
    let serializer = GraphFormat::GBZ.get_serializer();
    let output = File::create("graph.gbz")?;
    serializer.serialize(&graph, output)?;
    
    Ok(())
}
```

#### Work with De Bruijn Graphs

```rust
use pangraphx_core::de_bruijn_conversion::DeBruijn;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut file = std::fs::File::open("graph.vg")?;
    let parser = GraphFormat::VG.get_parser();
    let graph = parser.parse(&mut file)?;
    
    // Create de Bruijn graph with k=31
    let dbg = DeBruijn::from_vg(&graph, 31)?;
    
    println!("De Bruijn graph created with {} nodes", dbg.num_nodes());
    Ok(())
}
```

#### Compute Graph Statistics

```rust
use pangraphx_core::{CoreGraph, CoreGraphDTO, GraphFormat};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dto = CoreGraphDTO::load_from_file("graph.gfa", GraphFormat::GFA)?;
    let graph = CoreGraph::new(dto);
    let stats = graph.compute_stats();

    println!("Nodes: {}", stats.node_count);
    println!("Total sequence length: {} bp", stats.node_len.total);
    println!("N50: {} bp", stats.node_len.n50);
    println!("Connected components: {}", stats.component_count);
    Ok(())
}
```

## Project Structure

``` text
pangraphx-core/          # Core library crate
├── src/
│   ├── core/           # Internal graph representation (CoreGraph, CoreGraphDTO)
│   ├── formats/        # Format-specific parsers and serializers
│   ├── de_bruijn_conversion/  # De Bruijn graph algorithms
│   ├── traits.rs       # GraphParser and GraphSerializer traits
│   └── error.rs        # Error types
├── tests/              # Integration tests
└── Cargo.toml

pangraphx-cli/          # CLI tool crate
├── src/
│   ├── main.rs         # Entry point
│   ├── cli/            # CLI argument definitions
│   ├── handle_convert.rs  # Format conversion handler
│   ├── handle_info.rs     # Graph info handler
│   ├── handle_stats.rs    # Graph statistics handler
│   └── ...
└── Cargo.toml
```

## Building & Testing

### Build the Project

```bash
# Build both library and CLI
cargo build --workspace

# Build release version (optimized)
cargo build --workspace --release

# Build with ODGI support (Linux only)
cargo build --workspace --features pangraphx-core/odgi --release
```

### Run Tests

```bash
# Run all tests
cargo test --workspace

# Run tests for a specific package
cargo test --package pangraphx-core

# Run tests for a specific module
cargo test --package pangraphx-core -- formats::gfa_format
```

### Environment Setup

Enable debug logging:

```bash
RUST_LOG=debug pangraphx-cli convert -i graph.gfa -o graph.vg
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## References

- [GFA Format Specification](https://github.com/GFA-spec/GFA-spec)
- [Variation Graph Toolkit](https://github.com/vgteam/vg)
- [Genome Graphs and Indexing](https://www.ncbi.nlm.nih.gov/pmc/articles/PMC8289141/)
