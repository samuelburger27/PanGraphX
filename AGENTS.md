# PanGraphX

Rust workspace (edition 2024) for converting, inspecting and transforming
pangenome graphs. Two crates:

- `pangraphx-core` — the library: graph model, format codecs, statistics, de Bruijn construction.
- `pangraphx-cli` — the `pangraphx-cli` binary built on the library.

Supported formats: GFA 1.0 and 1.1 (walks are read, written as P lines), VG (protobuf), GBZ, ODGI (`.og`), FASTG.

## Git workflow

- **New feature or fix**: create a new branch from `main` whose name describes the request, and commit the work there.
- **Refining that work** (follow-ups, review fixes, tweaks): stay on the same branch and add commits to it.
- Commit with short, descriptive, imperative messages (e.g. `Reject VG node ids reused for different nodes`).
- **Never push.** Do not commit unrelated untracked files.

## Build, lint, test

```bash
cargo build --workspace
cargo build --profile dist                  # fat LTO, panic=abort → target/dist/pangraphx-cli (benchmarks)
cargo fmt --all --check
cargo clippy --all-targets --all-features   # must be clean
cargo test --workspace                      # all tests
cargo test -p pangraphx-core --test test_gfa
cargo bench -p pangraphx-core -- --test     # smoke-run the criterion benches
```

- `rustc` warnings and clippy `pedantic`, `nursery` and `perf` are **deny** at workspace level. Fix lints rather than silencing them; when an `#[allow]` is justified, scope it tightly and say why in a comment.
- `pangraphx-core/build.rs` compiles `proto/vg.proto` with `prost-build`, so `protoc` must be installed.
- CI (`.github/workflows/ci.yml`): fmt, clippy, test, cross-validate (vg/odgi), bench compile, coverage, `cargo audit`.

## Layout

```
pangraphx-core/src/
  lib.rs                 GraphFormat enum (dispatch, extensions, descriptions) + re-exports
  error.rs               PanGraphXError (thiserror) and PanResult<T>
  traits.rs              GraphParser<R: Read + Seek>, GraphSerializer
  proto_gen.rs           generated VG protobuf types
  core/
    core_types.rs        NodeId, Sequence, Orientation, Node, Edge, Step, Path, Nodes
    graph_dto.rs         CoreGraphDTO (transfer object), dangling_reference(), isomorphic()
    graph.rs             CoreGraph (mutable; adjacency list + path map)
    graph_utils.rs       path sequence helpers
    serialization.rs     CoreGraphDTO::load/load_from_file/save/save_to_file
    stats.rs             CoreGraph::compute_stats → GraphStats (N50, components, degrees, paths)
  formats/
    gfa_format.rs        GFACodec; parallel chunked writer
    gfa_parser.rs        in-house streaming two-pass GFA parser (no external GFA crate)
    vg_format.rs         VGCodec; libvgio framing, chunked Graph messages
    gbz_format.rs        GBZCodec via the `gbz-construct` crate; writes GBZ version 2 for vg compatibility
    odgi_format.rs       ODGICodec; native .og reader/writer (no odgi dependency)
    fastg_format.rs      FastgCodec; SPAdes/Bandage dialect, paths and overlaps dropped
    id_map.rs            external integer id → NodeId (dense table or hash map)
  de_bruijn_conversion/
    de_bruijn_graph.rs   DeBruijn (paths or full topology), compaction to DTO
    colored_dbg.rs       ColoredDBG (input paths re-walked through the k-mers on output)
    output.rs            streaming GFA / FASTA unitig output from the tables
    extraction.rs        streaming path / bidirected-topology k-mer walkers
    word.rs              2-bit u64/u128 words, RollingWindow
    kmer_set.rs          sharded concurrent k-mer set
    compaction.rs        unitig construction
    k_mers.rs            public Kmer type, validate_k (1..=63)
pangraphx-cli/src/
  main.rs                logging, global options (--threads, -v/-q), dispatch, error printing
  cli/args_parser.rs     clap: convert, dbg, info, stats, format (alias formats)
  graph_io.rs            load/save with format resolution; `-` = stdin (spooled to a temp file) / stdout
  handle_*.rs            one handler per subcommand
```

Other directories: `docs/odgi_format.md` (ODGI binary layout), `utils/` (fixture and tool scripts), `benchmarks/` (see its README), `pangraphx-core/benches/` (criterion), `temp/` (gitignored scratch).

## Data model

- `NodeId = usize` is always the node's index in `Nodes`; `Nodes` enforces this (`from_node_vec` validates, `push` assigns). Never use a raw `Vec<Node>`.
- Original node names live in the optional `node_name_map: HashMap<NodeId, NodeName>`; nodes without an entry are named by their id.
- `Sequence`, `NodeName`, `PathName` are `Vec<u8>`.
- `Edge { from_node, from_orient, to_node, to_orient, overlap }`. `a+ → b+` and `b- → a-` are the same link.
- `Step` packs a node and orientation into a `u32` (`id << 1 | is_reverse`). Build with `Step::new` (fails with `NodeIdTooLarge` above `Step::MAX_NODE_ID` = 2^31 − 1).
- `Path { name, steps, overlaps }`.
- `CoreGraphDTO` is what parsers return and serializers take. Its fields are public, so serializers call `dangling_reference()` once and then index nodes directly.
- `CoreGraph` is the mutable form (built with `CoreGraph::new(dto)`, back with `CoreGraphDTO::from`). Keep `adjacency_list` and `path_map` in sync when editing.

## Format notes

- **IDs**: VG and ODGI keep node ids only when every name is a distinct, positive, canonically written integer; otherwise nodes are numbered `1..=n`. `id_map.rs` is shared by VG, GBZ and numeric GFA names.
- **Fidelity**: VG drops per-step path overlaps; GBZ keeps only what its paths imply (no overlaps, no off-path or repeated edges); ODGI drops overlaps; FASTG drops paths and overlaps.
- **GFA walks**: `W` lines become paths named as vg names them (`gfa_parser::walk_path_name`, checked against vg 1.73); the writer emits P lines only. odgi ignores W lines and GBZ keeps only a walk's subrange start, so the `walks` fixture has only a VG reference file.
- **ODGI**: the `.og` layout is odgi-internal and unversioned. `docs/odgi_format.md` documents it; `utils/dump_odgi.py` dumps a file.
- **GBZ**: unlike `vg gbwt`, we do not chop nodes to 1024 bp, and all paths are written as generic paths.

## de Bruijn construction

- Nodes are distinct canonical k-mers, edges distinct canonical (k+1)-mers, stored as sorted 2-bit tables; node id = rank, so output is independent of thread count. `k ≤ 31` uses `u64` words, `k ≤ 63` uses `u128`.
- Modes: paths (default), full topology (`-t`, every walk of the bidirected graph), colored (`-c`, keeps paths); `-u/--compact` merges non-branching chains into unitigs.
- Memory is the constraint: keep per-k-mer state to a few bytes and never materialize per-path k-mer vectors. Output is dominated by uncompacted node count.
- Output: `dbg` writes GFA (or FASTA unitigs with `-u`) straight from the tables (`write_gfa`, `write_unitigs_fasta`); other formats are rejected since they drop the `k − 1` overlaps. Only compacted graphs go through a `CoreGraphDTO`.
- Colored paths are never stored: `ColoredDBG` keeps the input graph and the end k-mers of each gap-free segment, and every consumer re-walks the paths with `walk_segments`, so all passes agree on segmentation.

## Code style

- **Library errors**: return `PanResult<T>`. Add variants to `PanGraphXError` in `error.rs`; convert external errors (I/O, protobuf, gbz) into them. Malformed input is `Parse` with enough context (e.g. line number) to locate it.
- **No panics in the library**: no `unwrap`/`expect`/unchecked indexing on untrusted data; validate and return an error. Tests may unwrap.
- **CLI errors**: `anyhow::Result` in handlers with `.context(...)`; `main.rs` prints the chain. Usage errors go through clap.
- **Logging**: `log` macros (`debug!`, `info!`, `warn!`). Logs go to stderr; the CLI shows `info` (progress) by default, `-q` / `-v` change that and `RUST_LOG` overrides. stdout is for data only (`println!` only for actual CLI output), since `-o -` writes graphs there.
- **Safety**: safe Rust; any `unsafe` needs a measured reason and a `// SAFETY:` comment.
- **Performance**: large inputs (e.g. HPRC chr22) are the target. Stream I/O, avoid per-element allocation in hot loops, parallelize with `rayon`, and keep output deterministic.
- Match surrounding code: doc comments on public items (`# Errors` sections where clippy asks), module-level `//!` docs describing the algorithm or format.

## Adding a format

1. Add `formats/<name>_format.rs` with a codec struct implementing `GraphParser` and/or `GraphSerializer`.
2. Export it from `formats/mod.rs`.
3. Register it in `GraphFormat` (`lib.rs`): variant, `iter`, `Display`, extension mapping, parser/serializer, description.
4. Add a `Fidelity` constant in `tests/common/mod.rs`, round-trip tests, and fixtures.

## Testing

- Integration tests live in `pangraphx-core/tests/` (one `test_<format>.rs` per format, plus stats, graph manipulation, DBG oracle) and `pangraphx-cli/tests/cli.rs`.
- Compare graphs with `tests/common` (`canonical(graph, Fidelity::X)` + `assert_same` / `assert_equivalent`), not `CoreGraphDTO::isomorphic`. Writer tests round-trip in memory (`common::round_trip`).
- Fixtures: `tests/test_files/gfa/` is the source of truth. `vg/`, `gbz/`, `og/` and `MANIFEST.tsv` are written by real `vg`/`odgi` via `utils/generate_test_files.sh` — never by PanGraphX — and are checked tool-free by `test_reference_files.rs`. `utils/generate_gfa.py` generates synthetic GFA.
- DBG changes: `test_dbg_oracle.rs` checks construction against a naive oracle.
- After changing the VG, GBZ or ODGI codecs run
  `cargo test --release -p pangraphx-core --test cross_validation`
  with `vg` and `odgi` on `PATH` (or `PANGRAPHX_VG` / `PANGRAPHX_ODGI`). Missing tools print `SKIPPED`; `PANGRAPHX_REQUIRE_EXTERNAL=1` makes that a failure. Pinned versions are in `utils/install_external_tools.sh`.

## Benchmarks

- Criterion suites in `pangraphx-core/benches/`: `formats`, `de_bruijn`, `parallel`, `dbg_memory`.
- `benchmarks/compare_tools.sh` (time + peak RSS vs vg/odgi via hyperfine), `benchmarks/dbg.sh` (memory-guarded DBG sweep), `benchmarks/setup_ubuntu.sh` (fresh machine setup); helpers in `lib.sh`/`ops.sh`. Details in `benchmarks/README.md`.
- Large graphs go to gitignored `benchmarks/data/` via `utils/download_gfa.sh`.
- Use the `dist` build for timing. **Never run DBG benchmarks in parallel** — memory grows with the number of distinct k-mers.
