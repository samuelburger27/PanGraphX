# PanGraphX

A fast and efficient Rust library and command-line tool for working with pangenome graphs. PanGraphX provides seamless conversion between multiple genome graph formats and includes tools for de Bruijn graph construction and genome graph analysis.

## Features

- **Format Conversion**: Convert between GFA, GBZ, VG, and FASTG formats
- **Graph Inspection**: Query basic information about graph files
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

#### Docker (Recommended for ODGI Support)

Docker provides the easiest way to use PanGraphX with full ODGI support on any platform:

```bash
# Build the image
docker compose build

# Run a conversion (files go in the ./data directory)
docker compose run pangraphx convert -i /data/graph.gfa -o /data/graph.vg

# List supported formats
docker compose run pangraphx formats

# Or use docker directly
docker build -t pangraphx .
docker run -v $(pwd)/data:/data pangraphx convert -i /data/graph.gfa -o /data/graph.vg
```

Place your input files in the `data/` directory — it is mounted into the container at `/data`.

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
│   ├── convert.rs      # Format conversion handler
│   ├── handle_info.rs  # Graph info handler
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

### Docker

Build and test with full ODGI support using Docker:

```bash
# Build the Docker image
docker compose build

# Run tests inside the container
docker compose run pangraphx cargo test --workspace --features pangraphx-core/odgi
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
