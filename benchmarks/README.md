# PanGraphX benchmarks

| Suite | Kind | Measures |
|-------|------|----------|
| `formats` | criterion | Parse / serialize GFA·VG·GBZ, GFA↔VG conversion. CPU only, MB/s |
| `de_bruijn` | criterion | DBG construction time over *k*, and standard vs. colored vs. full-topology |
| `parallel` | criterion | Rayon thread scaling (1 → all cores) of DBG, GFA parse, GFA serialize |
| `dbg_memory` | custom bench | Peak **heap** of DBG construction over mode × *k* × threads (deterministic) |
| `compare_tools.sh` | shell + hyperfine | Wall-clock time and peak RSS of PanGraphX vs. `vg` / `odgi` conversions |
| `dbg.sh` | shell + `time` | End-to-end `pangraphx-cli dbg` time + peak RSS over mode × *k* × threads, memory-guarded |

The Criterion benches measure in-process CPU cost with disk I/O excluded. The
shell scripts measure whole CLI runs the way a user experiences them.

## Datasets

| Location | Contents |
|----------|----------|
| `pangraphx-core/tests/test_files/gfa/` | Small curated fixtures (phiX174, phage λ, *M. genitalium*, merged, small synthetic) |
| `benchmarks/data/` (gitignored) | Large graphs: `ecoli50`, `chr22.hprc-v1.1-mc-grch38` (fetch with `utils/download_gfa.sh`) and `random_medium` (60 MB synthetic, generated on demand by `utils/generate_gfa.py`) |

Only GFA files are inputs. The scripts look up a dataset name as `<name>.gfa`
in `benchmarks/data/` first, then in the fixtures directory, and they also
accept a path. A dataset that can't be found is skipped with a loud warning.
Every other format is derived from the GFA: in memory by the Criterion benches,
and by the reference tool for the shell scripts (see below).

## 1. Criterion benches

```bash
cargo bench -p pangraphx-core                      # everything
cargo bench -p pangraphx-core --bench formats      # one suite
cargo bench -p pangraphx-core -- --test            # smoke test, one iteration each

PANGRAPHX_BENCH_DATA_DIR=auto cargo bench ...      # fixtures + benchmarks/data/
PANGRAPHX_BENCH_DATA_DIR=/some/dir cargo bench ... # every *.gfa in /some/dir
```

HTML reports go to `target/criterion/report/index.html`.

**Size classes.** Criterion's sample size shrinks as the input grows, down to
its hard minimum of 10. Huge inputs also get 1 s measurement and warm-up times.

| Class | File size | Samples | Notes |
|-------|-----------|---------|-------|
| small | < 1 MiB | 100 | |
| medium | 1–64 MiB | 20 | |
| large | 64–256 MiB | 10 | |
| huge | ≥ 256 MiB | 10 | 1 s timing; streamed from disk instead of pre-read |

**DBG inputs.** DBG construction is gated on the **total path bases**, not on
file size. A 55 MB GFA whose paths reuse nodes can spell out 500 Mbp. The
default cap is 10 Mbp; change it with `PANGRAPHX_BENCH_DBG_MAX_BP`. Inputs:
- By default `de_bruijn` uses `lambda` and `mycoplasma`, and `parallel_dbg` uses
  `merged`.
- With `PANGRAPHX_BENCH_DATA_DIR` set, they use the largest input under the cap.

DBG construction consumes its input, so every sample clones a parsed fixture
(`iter_batched`, outside the timed region). *k* ≤ 63, since k-mers are 2-bit
packed into a `u128`.

### `dbg_memory`

```bash
cargo bench -p pangraphx-core --bench dbg_memory
DBG_MEMORY_K="31" DBG_MEMORY_THREADS="1 8" PANGRAPHX_BENCH_DATA_DIR=auto \
    cargo bench -p pangraphx-core --bench dbg_memory
```

This bench swaps in a peak-tracking global allocator. Modes are `standard`,
`colored`, `full_topology`, `compact` and `colored_compact` (the last two use
unitig output). Each configuration runs one at a time and reports heap bytes:

| Column | Meaning |
|--------|---------|
| input | the parsed input graph |
| build peak | peak during construction |
| retained | heap the finished DBG holds |
| total peak | peak including conversion to a `CoreGraphDTO` (or compaction) |
| B/bp | total peak per path base |

Heap bytes don't depend on the OS or the allocator's page reuse, which makes
this the number to track when optimizing memory. The CSV goes to
`target/dbg_memory.csv`.

## 2. External-tool comparison: `compare_tools.sh`

| Operation | PanGraphX | Reference tool |
|-----------|-----------|----------------|
| GFA → VG | `convert g.gfa -o g.vg` | `vg convert --gfa-in --vg-out g.gfa` |
| VG → GFA | `convert g.vg -o g.gfa` | `vg convert --gfa-out g.vg` |
| GFA → GBZ | `convert g.gfa -o g.gbz` | `vg gbwt -G g.gfa -g g.gbz --max-node 0 --num-jobs N` |
| GBZ → GFA | `convert g.gbz -o g.gfa` | `vg convert --gfa-out g.gbz` |
| GFA → OG | `convert g.gfa -o g.og` | `odgi build` |
| OG → GFA | `convert g.og -o g.gfa` | `odgi view -g` |

The table is defined once, in `ops.sh`. Before measuring a dataset, the script
builds the reverse-direction inputs (`.vg`, `.gbz`, `.og`) once with the
**reference tool**, untimed. Both tools then read the same files. Pass
`WORK=/some/dir` to keep those inputs and reuse them across runs.

`odgi build` and `odgi view` get `-t N` (all cores, override with
`ODGI_THREADS`); without it odgi runs on one thread.
`vg convert` gets no `-t`: it already defaults to all cores, and GFA ↔ VG
conversion is single-threaded anyway.
`vg gbwt` gets all cores (`N`, override with `VG_THREADS`): it has no
`-t`/`--threads`; `--num-jobs` is its thread setting for GFA input, and `-g`
(not `-o`, which writes a bare GBWT) produces the GBZ. `--max-node 0` stops
vg from chopping segments into nodes of at most 1024 bp, so both tools build
a GBZ over the same nodes as the GFA. vg writes `.vg` files
as Protobuf (`--vg-out`), the encoding PanGraphX reads and writes, so GFA → VG
compares the same output format.

```bash
benchmarks/compare_tools.sh                         # default datasets (lib.sh)
benchmarks/compare_tools.sh mycoplasma_genitalium ecoli50
OPS="gfa_to_vg vg_to_gfa" RUNS=20 benchmarks/compare_tools.sh merged
```

How sampling works:
- hyperfine's sample count scales with input size: 20 runs under 1 MiB, down to
  3 runs at 256 MiB and above. `WARMUP` / `RUNS` override this.
- hyperfine ≥ 2 records the peak RSS of every run alongside its wall-clock
  time (`getrusage` of the child, the same number `time -l` / `time -v`
  reports). `summary.md` gets a time table and a peak-RSS table (median and
  mean, MiB) per comparison; the raw per-run values are in the JSON.

Prerequisites:
- `hyperfine` ≥ 2 (`cargo install hyperfine`)
- `vg` (bioconda or GitHub release)
- `odgi` (bioconda or built from source)

On Ubuntu, `setup_ubuntu.sh` installs all of these.