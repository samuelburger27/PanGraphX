# PanGraphX benchmarks

| Suite | Kind | Measures |
|-------|------|----------|
| `formats` | criterion | Parse / serialize GFA·VG·GBZ, GFA↔VG conversion. CPU only, MB/s |
| `de_bruijn` | criterion | DBG construction time over *k*, and standard vs. colored vs. full-topology |
| `parallel` | criterion | Rayon thread scaling (1 → all cores) of DBG, GFA parse, GFA serialize |
| `dbg_memory` | custom bench | Peak **heap** of DBG construction over mode × *k* × threads (deterministic) |
| `compare_tools.sh` | shell + hyperfine | Wall-clock time of PanGraphX vs. `vg` / `odgi` conversions |
| `peak_memory.sh` | shell + `time` | Peak RSS for the same conversions |
| `dbg.sh` | shell + `time` | End-to-end `pangraphx-cli ddb` time + peak RSS over mode × *k* × threads, memory-guarded |

The Criterion benches measure in-process CPU cost with disk I/O excluded. The
shell scripts measure whole CLI runs the way a user experiences them.

## Datasets

| Location | Contents |
|----------|----------|
| `pangraphx-core/tests/test_files/gfa/` | Small curated fixtures (phiX174, phage λ, *M. genitalium*, merged, synthetic) |
| `benchmarks/data/` (gitignored) | Large graphs: `ecoli50`, `chr22.hprc-v1.1-mc-grch38`. Fetch with `utils/download_gfa.sh` |

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

This bench swaps in a peak-tracking global allocator. Each configuration runs
one at a time and reports heap bytes:

| Column | Meaning |
|--------|---------|
| input | the parsed input graph |
| build peak | peak during construction |
| retained | heap the finished DBG holds |
| total peak | peak including conversion to a `CoreGraphDTO` |
| B/bp | total peak per path base |

Heap bytes don't depend on the OS or the allocator's page reuse, which makes
this the number to track when optimizing memory. The CSV goes to
`target/dbg_memory.csv`.

## 2. External-tool comparison: `compare_tools.sh`, `peak_memory.sh`

| Operation | PanGraphX | Reference tool |
|-----------|-----------|----------------|
| GFA → VG | `convert -i g.gfa -o g.vg` | `vg convert --gfa-in --vg-out` |
| VG → GFA | `convert -i g.vg -o g.gfa` | `vg convert --gfa-out` |
| GFA → GBZ | `convert -i g.gfa -o g.gbz` | `vg gbwt -G g.gfa -g g.gbz` |
| GBZ → GFA | `convert -i g.gbz -o g.gfa` | `vg convert --gfa-out` |
| GFA → OG | `convert -i g.gfa -o g.og` | `odgi build` |
| OG → GFA | `convert -i g.og -o g.gfa` | `odgi view -g` |

The table is defined once, in `ops.sh`, and both scripts use it. Before
measuring a dataset, each script builds the reverse-direction inputs (`.vg`,
`.gbz`, `.og`) once with the **reference tool**, untimed. Both tools then read
the same files. Pass the same `WORK=/some/dir` to both scripts to reuse those
inputs across them.

```bash
benchmarks/compare_tools.sh                         # default datasets (lib.sh)
benchmarks/compare_tools.sh mycoplasma_genitalium ecoli50
OPS="gfa_to_vg vg_to_gfa" RUNS=20 benchmarks/compare_tools.sh merged
MEM_REPS=5 benchmarks/peak_memory.sh merged
```

How sampling works:
- hyperfine's sample count scales with input size: 20 runs under 1 MiB, down to
  3 runs at 256 MiB and above. `WARMUP` / `RUNS` override this.
- `peak_memory.sh` reports the median of `MEM_REPS` runs (default 3).
- hyperfine ≥ 2 also prints memory, but `peak_memory.sh` also works with
  hyperfine 1.x and writes a CSV.

Prerequisites:
- `hyperfine`
- `vg` (bioconda or GitHub release)
- `odgi` (bioconda, Linux; PanGraphX is then built with `--features odgi`)
- for peak RSS: GNU time on Linux (`apt install time`). On macOS the built-in
  `/usr/bin/time -l` is used.

A missing external tool is skipped with a warning. A failing step is written to
`failures.txt` and the run carries on. Common variables:

| Variable | Effect |
|----------|--------|
| `PANGRAPHX_BIN` | benchmark a pre-built binary |
| `SKIP_BUILD=1` | reuse `target/release/pangraphx-cli` |
| `RESULTS_DIR` | write results here instead of a new run directory |
| `WORK` | keep intermediate files in this directory |

## 3. DBG CLI benchmark: `dbg.sh`

```bash
benchmarks/dbg.sh mycoplasma_genitalium merged
K_VALUES="31" THREADS="1 2 4 8" MODES="standard colored" benchmarks/dbg.sh ecoli50
```

This runs `pangraphx-cli ddb` for each dataset × mode × *k* × thread count
(`RAYON_NUM_THREADS`). For every configuration it records peak RSS and wall
time, both to `dbg.csv` and to `dbg.md`.

### DBG memory: why it's guarded

DBG construction memory grows with total **path bases**, and **with thread
count**. Measured at k = 31 on a 10 MB synthetic GFA with 32 Mbp of paths:

| Threads | Construction heap | per path base |
|--------:|------------------:|--------------:|
| 1 | 5.7 GiB | 188 B |
| 2 | 6.8 GiB | 222 B |
| 4 | 11.4 GiB | 376 B |
| 8 | 14.5 GiB | 478 B |

The CLI end-to-end adds the DTO conversion and the output write, about
280 B/bp RSS on one thread. So:

- **Never run DBG benchmarks in parallel** with each other or with other
  benchmarks. `dbg.sh` runs one configuration at a time.
- **Pre-flight estimate.** `pangraphx-cli stats` gives the total path bp. The
  estimate is `3 × file + bp × DBG_BYTES_PER_BP × (1 + 0.25 × (threads − 1))`,
  with a default of 300 B/bp. Configurations whose estimate exceeds
  `DBG_MEM_BUDGET_MIB` (default 60 % of RAM) are recorded as
  `skipped-estimate`.
- **Watchdog.** Every run is polled every 0.2 s:
  - A run whose RSS goes over the budget is killed and recorded as `oom-guard`.
    The larger thread counts of that mode and *k* are then skipped.
  - A run that exceeds `DBG_TIMEOUT_S` (default 1800) is recorded as `timeout`.
  - Polling lets a fast allocation burst overshoot a little, so keep the budget
    well below physical RAM.
- **Full topology.** It enumerates graph walks and can blow up independently
  of path length. It is skipped above `DBG_FULL_TOPOLOGY_MAX_BP` (default
  5 Mbp) unless `DBG_ALLOW_FULL_TOPOLOGY=1` is set.

### Ideas for reducing DBG memory (future work)

1. Store `k` once per graph instead of in every `Kmer`. That makes `Kmer` 16 B
   (`u128`) instead of 32 B, or 8 B (`u64`) when k ≤ 32. `OrientedKmer` and
   `DbgEdge` shrink with it.
2. Stream path k-mers straight into the sets instead of first collecting
   `HashMap<&Path, Vec<OrientedKmer>>` for **all** paths. That vector alone is
   ~48 B per path base.
3. Replace the overlapping per-split `fold`/`reduce` sets with one sharded
   concurrent set, or partition k-mers by hash across threads. This removes
   the growth with thread count shown above.
4. Pack the orientation into a spare bit of the code.
5. Use sort + dedup on `Vec`s (or a radix sort) instead of `HashSet`s, to avoid
   the hash-table overhead.
6. In `From<DeBruijn> for CoreGraphDTO`, build the node sequences in place
   instead of going through a `HashMap<Kmer, Node>` and cloning every sequence.

`cargo bench --bench dbg_memory` and `dbg.sh` before and after a change show
the effect.

> **Caveat (as of this writing).** Path-based extraction
> (`extract_kmers_from_path`) restarts the k-mer window at every node, so no
> k-mer spans a node boundary. Graphs with short nodes therefore produce few
> or no k-mers: phage λ's nodes average 32 bp, and at k = 63 its DBG is empty.
> Their path-mode memory numbers are too low. Expect them to rise once
> extraction spans nodes.

## Results layout

Each script run writes a new directory, and `latest` points at the most recent
one:

```
benchmarks/results/                    (gitignored)
  compare_tools/<YYYYmmdd-HHMMSS>-<host>/
      env.txt          machine, commit (+dirty), tool versions, env
      summary.md       every table, ready for the thesis
      <op>_<ds>.md     per comparison (hyperfine Markdown)
      <op>_<ds>.json   raw hyperfine samples
      failures.txt     only if something failed
  peak_memory/<run>/   peak_memory.md, peak_memory.csv, env.txt
  dbg/<run>/           dbg.md, dbg.csv, env.txt
  <suite>/latest -> most recent run
  legacy/              results from before this layout
```

## Files

| File | Role |
|------|------|
| `lib.sh` | Shared helpers: paths, dataset lookup, tool discovery, build, run directories, sampling, `measure` (peak RSS + watchdog) |
| `ops.sh` | The conversion-operation table + `prepare_inputs` |
| `compare_tools.sh`, `peak_memory.sh`, `dbg.sh` | Entry points |
| `../pangraphx-core/benches/` | Criterion benches + `dbg_memory`; shared helpers in `common/mod.rs` |
