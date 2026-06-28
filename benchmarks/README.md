# PanGraphX benchmarks

This directory holds the benchmark suite used to evaluate PanGraphX for the
thesis. It has two complementary parts:

1. **Criterion micro-benchmarks** (in `pangraphx-core/benches/`) — statistically
   rigorous, in-process timing of the core operations, with disk I/O excluded.
2. **External-tool comparison** (the shell scripts here) — wall-clock and peak
   memory of the PanGraphX CLI versus the established tools `vg` and `odgi` on
   equivalent format conversions.

All inputs come from the repository's existing test data under
`pangraphx-core/tests/test_files/` (real genomes: phiX174, E. coli phage λ,
*Mycoplasma genitalium*, plus a merged multi-genome graph).

## 1. Criterion micro-benchmarks

Run from the repository root:

```bash
# Full run (writes HTML reports + raw data under target/criterion/)
cargo bench -p pangraphx-core

# A single suite
cargo bench -p pangraphx-core --bench formats
cargo bench -p pangraphx-core --bench de_bruijn
cargo bench -p pangraphx-core --bench parallel

# Quick smoke test (one iteration each, no measurement)
cargo bench -p pangraphx-core -- --test
```

Open `target/criterion/report/index.html` for the plots.

Suites:

| Bench file    | Groups                                   | Measures |
|---------------|------------------------------------------|----------|
| `formats`     | `parse`, `serialize`, `convert`          | Per-format parse (GFA/VG/GBZ), serialize (GFA/VG), end-to-end GFA↔VG. Throughput in MB/s. |
| `de_bruijn`   | `dbg_vs_k`, `dbg_modes`                   | DBG construction as a function of *k*; standard vs. full-topology vs. colored. |
| `parallel`    | `parallel_dbg`, `parallel_parse`, `parallel_serialize` | Rayon thread-scaling (1 → all cores) for DBG, GFA parse, GFA serialize. |

Notes:
- GBZ is a load-only format, so it appears only under `parse`.
- DBG construction consumes its input, so benches clone a parsed fixture per
  sample (`iter_batched` / `BatchSize::LargeInput`); cloning is not measured.
- *k* is capped at 63 (k-mers are 2-bit packed into a `u128`).

## 2. External-tool comparison (Linux)

`compare_tools.sh` uses [hyperfine](https://github.com/sharkdp/hyperfine) to
compare equivalent conversions:

| Operation | PanGraphX            | External tool            |
|-----------|----------------------|--------------------------|
| GFA → VG  | `convert -i g.gfa -o g.vg` | `vg convert --gfa-in --vg-out` |
| VG → GFA  | `convert -i g.vg -o g.gfa` | `vg convert --gfa-out`         |
| GFA → OG  | `convert -i g.gfa -o g.og` | `odgi build`                   |
| OG → GFA  | `convert -i g.og -o g.gfa` | `odgi view`                    |

`peak_memory.sh` measures peak RSS for the same operations via GNU `time -v`
(hyperfine times only).

### Prerequisites

- **hyperfine** — `cargo install hyperfine`, or `apt install hyperfine` /
  `conda install -c conda-forge hyperfine`.
- **vg** — release binary from <https://github.com/vgteam/vg/releases>, or
  `conda install -c bioconda vg`.
- **odgi** — `conda install -c bioconda odgi` (Linux). Required both as a CLI
  for the comparison and, for PanGraphX's own `.og` I/O, the `odgi` cargo
  feature (the script builds `--features odgi` automatically when `odgi` is on
  PATH).
- **GNU time** (for `peak_memory.sh`) — `apt install time` on Debian/Ubuntu.

Any external tool that is missing is skipped with a warning, so the suite still
runs (e.g. on macOS, where odgi is unavailable).

### Running

```bash
# Default datasets: lambda, mycoplasma, merged
benchmarks/compare_tools.sh
benchmarks/peak_memory.sh

# Custom datasets (base filenames present under test_files/{gfa,vg,og}/)
benchmarks/compare_tools.sh mycoplasma_genitalium merged

# Tune sampling (compare_tools.sh)
WARMUP=5 RUNS=20 benchmarks/compare_tools.sh
```

Results are written to `benchmarks/results/`:
- `*.md` — Markdown tables (drop straight into the thesis).
- `*.json` — raw hyperfine samples for custom plots.
- `peak_memory.md` — RSS comparison table.

For headline numbers, point the scripts at a larger real pangenome by placing it
under `test_files/` and passing its base name as an argument.
