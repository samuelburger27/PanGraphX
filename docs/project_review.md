# PanGraphX — Project Review

*Reviewed state: `main` @ `e3a6b69` (2026-10-20). Scope: the whole repository
(library, CLI, tests, CI, benchmarks, docs). The thesis text was not available,
so the assessment covers the software artefact only.*

All 161 tests pass (`cargo test --workspace --release`, ~29 s, with `odgi` on
`PATH`). Every finding marked **verified** below was reproduced with the
`target/release` binary and, where relevant, `vg v1.73` / `odgi v0.9.4`.

---

## 1. Summary

PanGraphX is a well-engineered, performance-conscious Rust toolkit. Its
strongest parts go well beyond what a bachelor thesis requires: an in-house
streaming GFA parser, a native reader and writer for odgi's undocumented `.og`
layout (with a written spec), GBZ *construction* added to a `gbwt-rs` fork, and
a memory-lean de Bruijn pipeline. The engineering hygiene is also strong:
pedantic clippy treated as errors, CI with coverage and `cargo audit`, and
reproducible benchmarks against vg and odgi.

The main weakness is **validation**. Almost all format tests are
*self round-trips*: PanGraphX writes a file and PanGraphX reads it back. A
symmetric bug, where the writer and the reader share the same misunderstanding,
or an encoding that only goes wrong for some inputs, passes unnoticed. The
review found one silent data-loss bug of this kind (VG node-id collision,
§3.1), and both directions of GFA 1.1 `W`-line handling are missing (§3.2).
The VG and GBZ fixtures built by vg are generated but never read by any test,
and CI never runs any external tool.

You are right that the integration tests are the weakest part. Automated
cross-validation is the highest-value improvement, and §5 proposes a concrete
design for it.

---

## 2. Strengths

### 2.1 Architecture and scope
- **Clear layering.** `CoreGraphDTO` is the neutral interchange model, each
  format is one codec behind `GraphParser`/`GraphSerializer`, `GraphFormat`
  does the dispatch, and the CLI is a thin layer. Adding a format takes four
  documented steps.
- **Broad format coverage.** GFA 1.0, VG (protobuf), GBZ, ODGI and FASTG,
  read and write, with no C/C++ dependency. The ODGI codec reimplements a
  binary layout that odgi itself does not document; `docs/odgi_format.md` and
  `utils/dump_odgi.py` make this a research contribution in its own right.
- **GBZ construction.** The upstream `gbz` crate can only read GBZ. Writing it
  required extending `gbwt-rs` (GBWT builder plus graph builder), and the
  output is pinned to format version 2 so that current vg can read it.

### 2.2 Performance engineering
- **The GFA parser** reads the input in two passes of bounded blocks, so the
  file text is never fully resident. Records are parsed in parallel, and long
  `P` lines are split into pieces that are parsed in parallel. Numeric names go
  through a dense `IdMap` without hashing, and errors report line numbers.
- **Compact data model.** `Step` is a packed 4-byte handle (`id << 1 | rev`).
  Serializers check for dangling references once, then index directly. Output
  is written in bounded parallel windows, so memory stays flat.
- **The de Bruijn pipeline** uses 2-bit `u64`/`u128` words, a rolling window,
  a sharded concurrent k-mer set, sorted tables where the node id is the rank,
  and per-k-mer bitmask compaction. Output does not depend on the thread count
  (there is a test for this).
- **Measured results.** On chr22 (HPRC MC), GFA→VG takes 2.6 s vs 277 s for
  vg, VG→GFA 5.8 s vs 38 s, and GFA→GBZ 25.9 s vs 40 s. GBZ→GFA is on par
  (1.96 s vs 1.69 s). The benchmark notes are honest about startup floors,
  thread counts and vg's GBZ chopping.

### 2.3 Engineering hygiene
- Workspace lints deny `warnings`, `clippy::pedantic`, `nursery` and `perf`.
  This is unusually strict and it shows in the code.
- CI runs fmt, clippy, tests, a bench compile, llvm-cov coverage, and a weekly
  `cargo audit`.
- The library does not panic (no `unwrap` in library code) and has typed
  errors via `thiserror`. Logging uses `log`/`env_logger`.
- Good rustdoc in most modules, with doctests on the `CoreGraph` API.
  `benchmarks/README.md` and `setup_ubuntu.sh` make results reproducible on a
  fresh machine.

### 2.4 Correctness evidence that already exists
- `RollingWindow` is compared with a naive implementation.
- `test_odgi.rs` checks real interoperability: `odgi validate` and `odgi view`
  on our files, and parsing of files written by `odgi build`.
- **Independent DBG check (done during this review).** A 40-line Python oracle
  spelled every path, collected canonical k-mers and (k+1)-mers, and compared
  them with `ddb` output. Node sets and edge counts matched exactly for phiX,
  λ and `random` at k = 5, 21 and 31.
- **Ad-hoc vg cross-check (done during this review).** For all fixtures except
  `tiny` (vg rejects overlaps), both directions agree with vg once names are
  normalised: PanGraphX→`vg view` and `vg convert -v`→PanGraphX for VG, and
  PanGraphX→`vg convert -f` and `vg gbwt`→PanGraphX for GBZ. The core codecs
  are therefore sound; the problems are at the edges.

---

## 3. Weaknesses and bugs

### 3.1 VG writer silently merges nodes (verified, data loss)
`VgWriteContext::new` (`vg_format.rs`) assigns each node `name.parse::<i64>()`,
or `index + 1` when the name is not numeric. **Per node.** In a graph that mixes
numeric and non-numeric names, the two schemes collide:

```
S  2  AAAA          (index 0 → VG id 2, from its name)
S  x  CCCC          (index 1 → VG id 2, from index+1)
```

`pangraphx-cli convert mixed.gfa -o mixed.vg` exits with status 0. Reading the
file back yields **one** node and the path `2+,2+`. `vg validate` reports
"valid", and `vg view` shows node 2 = `CCCC`. On the read side,
`proto_nodes.dedup_by_key(id)` then drops duplicates without comparing
sequences, so the "Duplicate VG node ID" error path can never trigger. Zero and
negative names (VG ids must be ≥ 1) are accepted the same way. GBZ, ODGI and
FASTG handle the same input correctly. ODGI renumbers all-or-nothing, which is
the right model for VG as well.

**Fix:** keep the original ids only when *all* names are distinct positive
integers; otherwise renumber `1..=n` and store the original name. On read,
treat a repeated id with a different sequence as an error.

### 3.2 GFA 1.1 `W` lines are ignored (verified)
The README claims "GFA v1.1 with local extensions", but `W` records are skipped
with a `debug!` message only. A GFA whose haplotypes are walks, which is what
vg, minigraph-cactus and HPRC release 2 produce, converts with exit code 0 and
**zero paths**. The writer has the matching gap: GBZ→GFA produces
`P HG002#1#chr1#0` instead of `W` lines.

Related to this, PanSN metadata is lost on write: every path stored in GBZ
becomes `GENERIC` with `NO_SAMPLE_NAME`, as `vg paths -M` confirms. This is
already noted in the project memory as a benchmark-equivalence caveat.

**Fix:** parse `W` into `Path` with a structured name (sample, haplotype,
sequence id, range), and keep that metadata in the DTO so that GBZ can write
reference and haplotype paths. At the very least, emit a `warn!` and count the
skipped records.

### 3.3 Smaller correctness issues
| Where | Issue |
|---|---|
| GFA `S * LN:i:5` | `*` is stored as the literal 1-byte sequence `*`. `stats` reports 1 bp, and the sequence leaks into VG/ODGI/DBG output. Either reject it or model "unknown sequence" explicitly. (verified) |
| VG edges | `pe.overlap as u32` wraps negative values. Edges repeated across chunks are not deduplicated (nodes are). |
| GFA paths | A mismatch between the overlap count and the step count only warns, and the inconsistent path is kept. |
| `CoreGraphDTO::isomorphic` | The name is misleading: it is a *name-based* equality. It compares edges in their stored direction, so `a+→b+` ≠ `b-→a-`, compares edges as a set (multi-edges are not counted), and ignores path overlaps. Every test file works around this with its own canonicalisation. |
| VG type tag | A message is detected as a type tag by a heuristic (≤ 16 printable bytes). The first message could instead be checked against known tags (`"VG"`, …). |

### 3.4 Testing (the main gap)
1. **Self-referential round-trips.** `test_gfa`, `test_vg`, `test_gbz` and
   `test_fastg` only check `parse(write(g))`. This catches asymmetric bugs, but
   not conventions that the writer and reader share and get wrong, or lossy
   mappings that happen to round-trip (§3.1 round-trips "successfully" to the
   wrong graph).
2. **External fixtures are never consumed.** `generate_test_files.sh
   --external` builds `.vg` and `.gbz` files with vg, but no test reads
   `test_files/{vg,gbz,og}`. The only external file in use is `odgi/tiny.og`.
3. **CI never cross-validates.** CI does not install vg or odgi, and
   `test_odgi_cross_validation` silently returns when `odgi` is missing. The
   test is green whether or not anything was checked.
4. **Weak and duplicated equivalence checks.** `gbz_equivalent` compares the
   undirected adjacency and ignores orientations, so a bug that flips an edge
   orientation in GBZ would pass. `name_to_seq`, `edge_set`,
   `with_canonical_edges` and similar helpers are copied across files instead
   of living in `tests/common`.
5. **Fixture diversity is low.** Four of the seven GFAs are single-path linear
   chains (phiX, λ, *M. genitalium*, merged) with no reverse links. Only the
   synthetic `random*` files have bubbles and inversions. Missing cases: a real
   pangenome region (PGGB / minigraph-cactus), `W` lines, PanSN names,
   self-loops, multi-edges, nodes longer than 1024 bp (GBZ chopping), `N` and
   lowercase bases, `*` sequences, mixed numeric/non-numeric names, and the
   empty graph through every format.
6. **Loop-style tests.** Each test iterates over a directory with `println!`
   and stops at the first failing file. Per-case tests (`rstest`,
   `datatest-stable`) would report every failure separately.
7. **The CLI is barely tested.** There are 6 tests, mostly about clap errors.
   `info`, `stats` and `ddb` have no success-path tests, and
   `convert_takes_positional_input` writes to a fixed `/tmp` path.
8. **DBG has no integration test.** The unit tests are good, but nothing runs
   `ddb` end to end on a fixture or compares it with an oracle or an external
   tool. The check done in §2.4 should become a test.
9. **No property or fuzz testing**, despite the strict no-panic rule for
   parsers of untrusted binary input (VG, ODGI, GBZ).
10. A 60 MB `random_medium.gfa` is committed to git. It could be regenerated
    deterministically with `generate_gfa.py --seed`.

### 3.5 API and design
- **Two graph types with an unclear split.** `CoreGraph` stores paths in a
  `HashMap`, so the path order from the input is lost on `From<CoreGraph>`.
  Its adjacency is a `HashMap<NodeId, Vec<usize>>`, although node ids are
  dense, so a `Vec` (or CSR) would do. It only tracks outgoing edges in the
  stored direction, which is wrong for a bidirected graph: `has_edge(b, a)`
  misses `a+→b-`. `remove_edge` and `remove_node` clone *all node sequences*
  just to rebuild the adjacency list. The manipulation API is O(N + E + P) per
  operation and is not used by any command.
- **`node_name_map: Option<HashMap<NodeId, NodeName>>`** is a hash map keyed by
  a dense index. `Option<Vec<NodeName>>` would be smaller and faster, and every
  codec builds such a vector anyway (`node_names`).
- **Stringly-typed errors.** `Parse(String)`, `Other(String)` and
  `DeBruijn(String)` make it impossible for library users to match on causes.
  Variants such as `DanglingReference`, `DuplicateNode` or
  `UnsupportedRecord { format, line }` would help, and `GraphFormat` could be
  carried in the error.
- **Lossiness is undocumented in the API.** Each format drops something
  (overlaps, edge direction, off-path edges in GBZ, paths in FASTG, names in
  ODGI), but this is spread across READMEs and test comments. A per-format
  `capabilities()` (supports overlaps / names / off-path edges / path
  metadata), together with a CLI warning when a conversion is lossy, would make
  this explicit, and the tests could reuse it (§5.2).
- **`gbz` is a git dependency** on a personal fork branch. The crate cannot be
  published, and builds depend on that branch staying put. Pin a `rev` and
  consider upstreaming the builder to `jltsiren/gbwt-rs`.

### 3.6 CLI and documentation
- Progress messages (with emoji) go to **stdout**, there is no `-`
  (stdin/stdout) support, and there is no `--threads` option. All three limit
  use in pipelines. Progress should go to stderr or through `log`, keeping
  stdout for data.
- The subcommand is called `ddb`. The conventional abbreviation is `dbg`; `ddb`
  looks like a typo.
- The README advertises **"Format Validation"**, but no validate command
  exists. The library examples call `serializer.serialize(&graph, output)` with
  an owned `File`, while the trait takes `&mut dyn Write`.
- Leftovers: `simplelog` is an unused dependency, `pangraphx-cli/src/utils.rs`
  is empty, `f.txt` is untracked, there is a `TODO` about `BufWriter` in
  `traits.rs`, and the `[[thesis-benchmarks]]` memory link is dangling.
- vg itself warns that `--vg-out` (protobuf) is deprecated. The thesis should
  say that `.vg` here means the *legacy protobuf* format, not vg's current
  HashGraph/PackedGraph, and could optionally add `vg convert -p` to the
  benchmarks.

---

## 4. How good is it as a bachelor thesis?

**Implementation: excellent for a bachelor level, roughly master's level in
places.** About 13 k lines of non-trivial Rust, five binary and text formats
with no C++ dependencies, a reverse-engineered binary format with a written
spec, an upstream-quality library extension (GBZ construction), and
measurements against the reference tools on human chromosome-scale data.
Performance is competitive with or better than vg on most conversions, which
can be defended in front of a committee. The code quality signals (strict
lints, CI, coverage, audit, reproducible benchmark setup) are well above what
bachelor projects usually show.

**What a reviewer will probe:**
1. *"How do you know the conversions are correct?"* Today the honest answer is
   "round-trips plus odgi checks on my machine". With the cross-validation
   harness in §5 it becomes "every fixture × format is checked against
   vg/odgi in CI", which is a much stronger position. Write it up as its own
   *Validation* chapter.
2. *"Real pangenome inputs?"* Missing `W` lines and PanSN metadata are visible
   gaps, since current HPRC releases use them. Either implement them or scope
   them out explicitly in the thesis.
3. *"Is the DBG correct, and how does it compare?"* Comparing against a naive
   oracle (cheap, already done ad hoc) and an external tool (BCALM2, GGCAT or
   Cuttlefish 2 for compacted unitigs) would answer both questions.
4. *Fair benchmarking.* The equivalence caveats already recorded (GBZ chopping,
   path metadata, deprecated protobuf output) should appear in the text next
   to the speed-ups. Without them, factors like "105× faster" invite scepticism.

**Overall:** a strong thesis artefact, likely in the top grade band *if* the
written part covers validation and limitations as carefully as the code covers
performance. Fixing §3.1 and §3.2 and adding §5 removes the most obvious lines
of attack.

---

## 5. Proposal: automated cross-validation

### 5.1 Principle
Use *both* directions with an independent tool, and compare **semantically
normalised** graphs, never bytes:

```
          ┌────────────── A: our writer, their reader ──────────────┐
in.gfa ──►│ px convert → X.fmt ──► vg/odgi → ref.gfa ──► normalise ─┼─► ==  normalise(project(in.gfa, fmt))
          │ vg/odgi build → X.fmt ──► px convert → out.gfa ─► norm. ┼─► ==
          └────────────── B: their writer, our reader ──────────────┘
```

Direction A would have caught §3.1, because vg sees a single node 2 = `CCCC`.
Direction B catches reader bugs on files we did not write ourselves.

### 5.2 Components

1. **`pangraphx-core/tests/common/mod.rs`: one canonical form.**
   ```rust
   pub struct CanonicalGraph {
       segments: BTreeMap<String, Vec<u8>>,           // name → upper-case sequence
       links:    BTreeMap<(String, bool, String, bool), u32>, // min(edge, mirror) → multiplicity
       paths:    BTreeMap<String, Vec<(String, bool)>>,
   }
   pub struct Fidelity { overlaps: bool, edge_names: bool, off_path_edges: bool, paths: bool }
   pub fn canonical(g: &CoreGraphDTO, f: Fidelity) -> CanonicalGraph;
   ```
   `Fidelity` describes what each format keeps (it could be the `capabilities()`
   from §3.5). For a format that renumbers nodes (ODGI, and VG through vg),
   names are resolved by a bijection derived from the sequences and the path
   order, or by comparing on `1..=n` after the same renumbering. All existing
   ad-hoc helpers and `isomorphic` in tests are replaced by this one function.
   Report a readable diff (the first differing segment, link or path), not
   just `false`.

2. **Golden reference files (always run, no tools needed).**
   `generate_test_files.sh --external` writes into a committed
   `tests/test_files/reference/{vg,gbz,og}/`, together with a
   `MANIFEST.tsv` (file, sha256, tool, version, command). A test parses every
   reference file and compares it with the canonical form of its source GFA.
   This covers direction B on every `cargo test`. Keep the files small: drop
   `random_medium` from the references.

3. **Live tool tests (direction A, run in CI).**
   - `tests/cross_validation.rs`: for each fixture × {VG, GBZ, ODGI}, write
     with PanGraphX, read with the tool (`vg view`, `vg convert -f`,
     `odgi view -g`, plus `vg validate` and `odgi validate`), and compare.
   - Discover tools through `PANGRAPHX_VG` / `PANGRAPHX_ODGI` (falling back to
     `PATH`). **Skip loudly**, and fail when `PANGRAPHX_REQUIRE_EXTERNAL=1`
     is set, so that CI cannot pass silently.
   - Generate the cases with `datatest-stable` (one test per file) or `rstest`.

4. **CI job.**
   ```yaml
   cross-validate:
     runs-on: ubuntu-latest
     env: { PANGRAPHX_REQUIRE_EXTERNAL: "1" }
     steps:
       - uses: actions/checkout@v7
       - uses: ./.github/actions/setup-system-deps
       - uses: dtolnay/rust-toolchain@stable
       - uses: Swatinem/rust-cache@v2
       - uses: mamba-org/setup-micromamba@v2
         with:
           environment-name: xv
           create-args: -c conda-forge -c bioconda vg=1.73 odgi=0.9
           cache-environment: true
       - run: micromamba run -n xv cargo test --release --test cross_validation --test test_odgi
   ```
   Pin the same versions that `setup_ubuntu.sh` uses, so that the thesis
   results and CI agree. A separate job can check that the committed golden
   files still match what the pinned tools produce, to detect drift.

5. **Property-based round-trips (`proptest`).** Generate small random
   bidirected graphs: 1–30 nodes, random names drawn from {numeric,
   alphanumeric, mixed}, reverse links, self-loops, paths that follow links,
   optional overlaps. Check
   `canonical(parse(write(g, F)), F.fidelity) == canonical(g, F.fidelity)` for
   every format. Shrinking turns a failure into a minimal two-node example;
   §3.1 is exactly what this finds within seconds. Optionally run the same
   graphs through vg and odgi in the cross-validation job.

6. **DBG oracle and external comparison.**
   - Move the naive oracle into a test: spell the paths, collect canonical
     k-mers and (k+1)-mers, and assert set equality with `DeBruijn` for every
     fixture and k ∈ {3, 5, 21, 31, 32, 33, 63}. This covers the `u64`/`u128`
     switch and palindromes. For `--compact`, check that the unitigs spell
     exactly the same k-mer set and that no unitig can be extended further.
   - For the thesis, compare `--compact` with BCALM2 or GGCAT on the same
     sequences, using the multiset of canonical unitig sequences. That
     comparison is only valid for path mode, because full-topology mode has
     no equivalent in those tools.

7. **Fuzzing (`cargo-fuzz`).** One target per parser (GFA, VG, ODGI, GBZ, FASTG),
   seeded with the fixtures. The invariant is "returns `Ok` or `Err`, never
   panics, never allocates without bound". This backs the "do not panic" rule
   in `CLAUDE.md`.

8. **CLI end-to-end tests.** Success paths for `convert` (every format pair on
   `tiny` and `random`), snapshot tests (`insta`) for `info` and `stats`
   output, `ddb` with and without `-c`/`--compact`, and `tempfile` instead of
   fixed paths.

9. **Measure test strength.** `cargo mutants` (`mutants.out` is already in
   `.gitignore`) on `formats/` before and after these changes. The mutation
   score is a good number to quote in the thesis.

### 5.3 New fixtures (small, committed)
- `bubbles.gfa`: SNP, indel and inversion bubbles, reverse links, a self-loop
  and a multi-edge.
- `walks.gfa`: GFA 1.1 with `W` lines and PanSN names (reference plus two
  haplotypes).
- `names_mixed.gfa`: numeric and non-numeric names, `0`, `007` (regression
  test for §3.1).
- `long_node.gfa`: a node longer than 1024 bp (GBZ/vg chopping).
- `special_bases.gfa`: lowercase, `N`, IUPAC codes and a `*` sequence with
  `LN`.
- A real pangenome region of about 50–200 kb, extracted with
  `odgi extract`/`vg chunk` from the chr22 HPRC graph.
- `empty.gfa`.

---

## 6. Prioritised action list

| # | Action | Effort | Impact |
|---|---|---|---|
| 1 DONE | Fix VG id collision (§3.1) and add the `names_mixed.gfa` regression test | S | Removes silent data loss |
| 2 DONE | `tests/common` canonical form, replacing the ad-hoc comparisons and `isomorphic` in tests | S–M | Basis for everything below |
| 3 DONE | Cross-validation test plus CI job with pinned vg/odgi and `REQUIRE_EXTERNAL` (§5.2.3–4) | M | Main thesis validation story |
| 4 DONE | Commit golden reference files and read them in tests (§5.2.2) | S | Cross-checks without tools |
| 5 DONE | DBG oracle test (§5.2.6) | S | Cheap, strong correctness evidence |
| 6 | Parse `W` lines; at least warn on skipped records (§3.2) | M | Real pangenome inputs |
| 7 | `proptest` round-trips (§5.2.5) | M | Finds the next §3.1 automatically |
| 8 | New fixtures (§5.3) | S | Coverage of edge cases |
| 9 | CLI: progress to stderr, `-` for stdio, `--threads`; fix README claims | S | Usability and honesty |
| 10 | Fuzz targets, `cargo mutants` score | S–M | Robustness and a metric for the thesis |
| 11 | Path metadata (PanSN → GBZ haplotype paths), `capabilities()` API | M–L | Format fidelity |
| 12 | Pin `gbz` rev, tidy `CoreGraph` (Vec adjacency, bidirected neighbours), typed errors | M | Library quality |

S = under half a day, M = 1–2 days, L = more.
