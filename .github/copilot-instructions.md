# PanGraphX Copilot Instructions

## Project Guidelines

PanGraphX is a Rust workspace consisting of a core library (`pangraphx-core`) and a CLI tool (`pangraphx-cli`) for converting, inspecting, and manipulating Pangenome Graph formats.

- **Key Crates**: `anyhow` (CLI error handling), `thiserror` (Library error definitions), `clap` (CLI argument parsing), `log` (logging).

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
  - `formats/`: Format-specific logic (GFA, GBZ, VG, FastG).
    - Each format has a codec struct (e.g., `GBZCodec`, `GFACodec`) implementing generic traits.
  - `de_bruijn_conversion/`: Algorithms for de Bruijn graph construction and k-mer processing.
  - `traits/`: Defines `GraphParser` and `GraphSerializer`.
- **`pangraphx-cli`**: The command-line interface.
  - `cli/args_parser.rs`: `clap` definitions for subcommands (`Convert`, `DBG`, `Info`, `Validate`, `Formats`).
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
  - The project interacts with standard bioinformatics formats (GFA v1.0, GFA v1.1 locally supported extensions, GBZ, VG).

## Security

- **Input Validation**: Validate file formats strictly against specifications during parsing.
- **Safe Rust**: Prefer safe Rust. Avoid `unsafe` blocks unless strictly necessary for performance in hot loops, and document safety contracts clearly.
