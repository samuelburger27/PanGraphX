# Benchmark analysis: PanGraphX vs. vg and odgi on `nymfe501`

Analysis of two `compare_tools.sh` runs on the faculty machine `nymfe501.fi.muni.cz`.
The raw results are in `benchmarks/results/compare_tools/` (gitignored), so the
numbers used here are repeated in the tables below.

| Run directory | Tools |
|---------------|-------|
| `nymfe_vg_incorrect_odgi` | vg: GFA ↔ VG, GFA ↔ GBZ |
| `nymfe_odgi` | odgi: GFA ↔ OG |

## Setup

| | |
|---|---|
| Host | `nymfe501.fi.muni.cz`, Linux 6.12 x86_64 |
| CPU | Intel Core i5-14500 (20 hardware threads), 31 GiB RAM |
| PanGraphX | `pangraphx-cli 0.1.0`, `dist` profile, rustc 1.94.1, all threads (`RAYON_NUM_THREADS` unset) |
| vg | v1.73.0 (`VG_THREADS=20`; `vg convert` takes no thread flag and runs its own default) |
| odgi | v0.9.4-92-gfb43f081 (`ODGI_THREADS=20`, i.e. `-t 20`) |
| hyperfine | 2.0.0: 20 runs (< 1 MiB inputs), 10 runs (medium), 3 runs and no warm-up (ecoli50, chr22) |

The two runs are one commit apart (`772ae20` and `88ef308`), and that commit
only changes the benchmark script. PanGraphX is the same code in both runs.

Datasets, smallest to largest (statistics from `pangraphx-cli stats`):

| Dataset | GFA size | Nodes | Edges | Node bp | Paths | Path bp | Path steps |
|---|---:|---:|---:|---:|---:|---:|---:|
| `phix` | 9.9 KB | 169 | 168 | 5.4 k | 1 | 5.4 k | 169 |
| `escherichia_phage_lambda` | 94 KB | 1,516 | 1,515 | 48.5 k | 1 | 48.5 k | 1,516 |
| `mycoplasma_genitalium` | 1.2 MB | 18,128 | 18,127 | 580 k | 1 | 580 k | 18,128 |
| `merged` (λ + *M. genitalium*) | 1.3 MB | 19,644 | 19,642 | 629 k | 2 | 629 k | 19,644 |
| `random_medium` (synthetic) | 59 MB | 50,000 | 234,280 | 50.0 M | 25 | 500 M | 0.50 M |
| `ecoli50` | 390 MB | 1,553,667 | 2,120,160 | 18.1 M | 50 | 250 M | 36.9 M |
| `chr22.hprc-v1.1-mc-grch38` | 659 MB | 1,279,333 | 1,780,739 | 54.0 M | 512 | 3.23 G | 67.8 M |

Every reverse-direction input (`.vg`, `.gbz`, `.og`) was built by the reference
tool, so both tools read identical files.

"Speedup" below is `other tool time / PanGraphX time`, so values above 1 mean
PanGraphX is faster. Times are means over the runs.

## Summary

- **Large graphs: PanGraphX wins on every operation that writes or reads VG/OG
  and on GFA → GBZ.** On ecoli50 and chr22 it is 1.4–148× faster than the reference tool
  and uses less memory on the writes it wins. The single exception is GBZ → GFA, where vg is faster (1.4–1.6×).
- **GFA → VG is the standout:** 100× (ecoli50) and 147× (chr22) faster than
  `vg convert`, with 18× and 28× less peak memory (548 MiB vs 9.8 GiB, 634 MiB
  vs 17.7 GiB).
- **GFA → OG: 2.4–2.9× faster than odgi on the three larger inputs**, with
  less memory.
- **OG → GFA: 2.3–2.5× faster than odgi on large inputs**, but PanGraphX uses
  1.3–1.5× *more* memory.
- **Small inputs (< ~1 MB of GFA, i.e. milliseconds) are a toss-up and
  sometimes favor the other tool.** PanGraphX has a fixed cost of about 20 ms and
  20 MiB whenever it reads a GFA, which dominates there (see
  [Observations](#observations)).

## 1. PanGraphX vs. odgi

### GFA → OG

| Dataset | PanGraphX | odgi | Speedup | PanGraphX RSS | odgi RSS |
|---|---:|---:|---:|---:|---:|
| phix | 23.2 ms | 14.1 ms | 0.61× | 20.1 MiB | 17.2 MiB |
| lambda | 21.1 ms | 19.7 ms | 0.93× (parity) | 20.5 MiB | 17.2 MiB |
| mycoplasma | 39.0 ms | 54.6 ms | 1.40× | 24.9 MiB | 22.5 MiB |
| merged | 45.6 ms | 55.5 ms | 1.22× (within noise) | 25.6 MiB | 26.8 MiB |
| random_medium | 129.8 ms | 312.9 ms | 2.4× | 108 MiB | 152 MiB |
| ecoli50 | 1.652 s | 4.790 s | 2.9× | 1050 MiB | 1206 MiB |
| chr22 | 2.538 s | 7.161 s | 2.8× | 1483 MiB | 1597 MiB |

### OG → GFA

| Dataset | PanGraphX | odgi | Speedup | PanGraphX RSS | odgi RSS |
|---|---:|---:|---:|---:|---:|
| phix | 1.6 ms | 4.4 ms | 2.9× | 4.2 MiB | 13.1 MiB |
| lambda | 5.6 ms | 4.9 ms | 0.88× (noise) | 4.5 MiB | 13.1 MiB |
| mycoplasma | 24.3 ms | 18.7 ms | 0.77× | 8.9 MiB | 17.7 MiB |
| merged | 19.6 ms | 21.3 ms | 1.09× (noise) | 9.2 MiB | 18.1 MiB |
| random_medium | 105.2 ms | 137.3 ms | 1.3× | 126 MiB | 82 MiB |
| ecoli50 | 1.511 s | 3.520 s | 2.3× | 864 MiB | 644 MiB |
| chr22 | 2.648 s | 6.722 s | 2.5× | 1334 MiB | 897 MiB |

Takeaways:

- PanGraphX is consistently ~2.3–2.9× faster than odgi once the graph is larger than a few hundred MB
  of GFA, in both directions.
- **Memory is the weak spot for reading OG.** From `random_medium` upward,
  PanGraphX's OG → GFA peaks 1.3–1.5× higher than odgi's (chr22: 1.33 vs
  0.90 GiB). odgi writes the GFA from its compact in-memory graph. PanGraphX
  first loads the whole graph into a `CoreGraphDTO` (one allocation per node
  sequence, a name table, 4-byte path steps); the GFA writer itself only holds
  one window of formatted chunks at a time. The DTO is therefore the best
  candidate for a memory optimization.
- For writing OG the picture is the opposite: PanGraphX is smaller and faster.

## 2. PanGraphX vs. vg

vg numbers come from `nymfe_vg_incorrect_odgi`. Speedup = vg time / PanGraphX time.

### GFA → VG

| Dataset | PanGraphX | vg | Speedup | PanGraphX RSS | vg RSS |
|---|---:|---:|---:|---:|---:|
| phix | 22.3 ms | 9.5 ms | 0.43× | 20.1 MiB | 26.7 MiB |
| lambda | 22.3 ms | 13.5 ms | 0.61× | 20.7 MiB | 29.2 MiB |
| mycoplasma | 35.8 ms | 77.7 ms | 2.2× | 26.9 MiB | 42.4 MiB |
| merged | 38.0 ms | 82.2 ms | 2.2× | 27.1 MiB | 44.5 MiB |
| random_medium | 142.0 ms | 1425 ms | 10.0× | 170 MiB | 314 MiB |
| ecoli50 | 1.279 s | 127.7 s | 99.8× | 548 MiB | 9802 MiB |
| chr22 | 1.831 s | 269.8 s | 147× | 634 MiB | 17701 MiB |

### VG → GFA

| Dataset | PanGraphX | vg | Speedup | PanGraphX RSS | vg RSS |
|---|---:|---:|---:|---:|---:|
| phix | 1.9 ms | 4.5 ms | 2.4× | 4.3 MiB | 26.3 MiB |
| lambda | 3.2 ms | 15.6 ms | 4.8× | 4.7 MiB | 26.6 MiB |
| mycoplasma | 28.6 ms | 26.2 ms | 0.92× (noise) | 9.1 MiB | 31.1 MiB |
| merged | 26.2 ms | 27.2 ms | 1.04× (noise) | 9.4 MiB | 31.1 MiB |
| random_medium | 173.1 ms | 309.3 ms | 1.8× | 140 MiB | 117 MiB |
| ecoli50 | 4.032 s | 22.689 s | 5.6× | 1354 MiB | 2476 MiB |
| chr22 | 7.153 s | 38.241 s | 5.3× | 2017 MiB | 4258 MiB |

### GFA → GBZ

| Dataset | PanGraphX | vg | Speedup | PanGraphX RSS | vg RSS |
|---|---:|---:|---:|---:|---:|
| phix | 25.6 ms | 19.8 ms | 0.77× | 20.7 MiB | 38.6 MiB |
| lambda | 29.2 ms | 19.4 ms | 0.66× | 21.5 MiB | 39.1 MiB |
| mycoplasma | 54.3 ms | 40.9 ms | 0.75× | 31.4 MiB | 51.6 MiB |
| merged | 59.3 ms | 44.8 ms | 0.76× | 31.6 MiB | 64.6 MiB |
| random_medium | 722.9 ms | 763.6 ms | 1.06× | 264 MiB | 423 MiB |
| ecoli50 | 17.484 s | 24.272 s | 1.39× | 1333 MiB | 1665 MiB |
| chr22 | 33.368 s | 36.303 s | 1.09× | 1369 MiB | 1847 MiB |

### GBZ → GFA

| Dataset | PanGraphX | vg | Speedup | PanGraphX RSS | vg RSS |
|---|---:|---:|---:|---:|---:|
| phix | 2.3 ms | 19.6 ms | 8.4× | 4.5 MiB | 35.6 MiB |
| lambda | 6.3 ms | 18.9 ms | 3.0× | 4.9 MiB | 41.8 MiB |
| mycoplasma | 23.8 ms | 33.4 ms | 1.4× | 9.3 MiB | 102.9 MiB |
| merged | 19.4 ms | 31.1 ms | 1.6× | 9.7 MiB | 111.0 MiB |
| random_medium | 162.5 ms | 166.0 ms | 1.02× (parity) | 156 MiB | 243 MiB |
| ecoli50 | 988.8 ms | 697.7 ms | 0.71× | 660 MiB | 658 MiB |
| chr22 | 1.531 s | 0.951 s | 0.62× | 673 MiB | 647 MiB |

Takeaways:

- **GFA → VG:** the gap grows with input size, from "vg is faster" on
  10–94 KB graphs to two orders of magnitude on the large ones. vg's memory
  at 9.8–17.7 GiB means it would not run on a smaller machine; PanGraphX
  stays under 650 MiB. It is worth
  stating the cause only after checking it (it is not measured here), but the
  scaling suggests `vg convert --gfa-in --vg-out` builds the whole graph in an
  in-memory handle graph before writing, while PanGraphX streams the file.
- **VG → GFA:** consistently 5× faster than vg on the large graphs and
  half the memory. Exception: `random_medium`, where PanGraphX peaks 20 % higher
  (140 vs 117 MiB).
- **GFA → GBZ:** the closest race. vg is 1.3–1.5× faster on the small
  inputs; the crossover is around `random_medium` (~parity), and PanGraphX is
  1.4× faster on ecoli50. On chr22 the lead is only 9 % (33.4 ± 1.2 s vs
  36.3 ± 0.04 s, 3 runs), small enough that I would not claim it. Memory is
  clearly better in every case (1.2–2× less).
  Both tools build the GBZ over the same unchopped nodes (`--max-node 0`), and vg gets
  20 jobs, so this is not a thread-count artifact.
- **GBZ → GFA:** the one place vg is clearly better on large graphs: 1.4×
  (ecoli50) and 1.6× (chr22) faster at essentially equal memory. Both finish in
  about 1–1.5 s for a 390–660 MB output, so a large share of the time is plausibly
  writing the GFA, but this is a guess; profiling would tell. For small graphs
  PanGraphX wins by up to 8× (process start-up dominated).

## Observations

1. **Fixed cost when reading a GFA.** PanGraphX takes 21–25 ms and about 20 MiB
   for phix and lambda on every operation that reads a GFA (→ VG, → GBZ,
   → OG), but 1.6–6 ms and 4–5 MiB for ones that read VG/GBZ/OG. The inputs
   are 10 KB and 94 KB, so this is overhead, not work. It is the reason vg and
   odgi win the smallest cases. The memory part has a clear cause: the GFA
   parser's `BlockReader` resizes its buffer to `BLOCK_SIZE` (16 MiB) and
   zero-fills it before the first read, whatever the file size
   (`gfa_parser.rs`). The RSS floor stays at ~20 MiB with
   `RAYON_NUM_THREADS=1`, so the thread pool is not the cause. Capping the
   block at the file size should remove most of the floor. The time part is
   machine-dependent: on an 8-core MacBook the same phix GFA → VG takes 5.2 ms
   (vs. 2.9 ms for VG → GFA), on nymfe 22 ms; page-faulting the 16 MiB
   buffer is the likely but unverified cause. Irrelevant for the real target
   sizes, but visible in every table.
2. **Small-input results are noise-limited.** Standard deviations are 20–50 %
   of the mean for sub-50 ms commands (e.g. 45.6 ± 7.2 ms for `merged`
   GFA → OG). Differences under ~30 % on
   those rows should be treated as ties; I marked the obvious ones above.
3. **Large inputs have only 3 runs and no warm-up.** Standard deviations are
   small (mostly < 5 %, at most 12 %), so the large-graph conclusions are solid, but with no
   warm-up the first run may hit a cold page cache. The noisiest large rows are
   chr22 GFA → VG with PanGraphX (± 12 %) and chr22 GFA → GBZ (± 1.2 s, the
   one where this matters, since the two tools are 9 % apart).
4. **Memory is the main PanGraphX weakness**, as the sections above show: the OG → GFA direction (1.3–1.5×
   odgi) and GBZ → GFA (parity with vg). VG → GFA on `random_medium` (1.2× vg)
   is the smaller case of the same thing: reading a format and serializing GFA in one go.
5. **Hardware threads vs. cores.** The i5-14500 has 14 cores (6 P + 8 E) and
   20 hardware threads. `20` is therefore the right "all threads" value.

## Conclusions for the thesis

- Headline claims that the data supports: GFA → VG 100–150× faster and 18–28×
  less memory than vg; GFA → OG ~2.8× faster than odgi on
  graphs of 400–700 MB; VG → GFA ~5× faster and about half the memory of vg.
- Claims to word carefully: GFA → GBZ ("comparable to vg, up to 1.4× faster, with
  less memory"), and everything under ~100 ms.
- Known losses to state honestly: GBZ → GFA on large graphs (vg 1.4–1.6× faster)
  and OG → GFA peak memory (1.3–1.5× odgi).
- Reproduce with `benchmarks/compare_tools.sh` on the same machine.
