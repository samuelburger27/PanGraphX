HPRC chr22, human genome graph:

https://zenodo.org/records/17173731?preview_file=chr22.zip



ecoli :

```bash
wget -c https://zenodo.org/record/7937947/files/ecoli50.gfa.zst
zstd -d ecoli50.gfa.zst
```



https://cortexassembler.sourceforge.net


Download them with [`utils/download_gfa.sh`](../../../utils/download_gfa.sh),
which puts them in `benchmarks/data/` (they are benchmark inputs, not test
fixtures).

The GFA files in `gfa/` are the source of every other test graph. The `vg/`,
`gbz/` and `og/` files are written from them by the external tools themselves
(`vg convert`, `vg gbwt --max-node 0`, `odgi build`), never by PanGraphX, for
the small fixtures. They are committed so that `tests/test_reference_files.rs`
checks our readers against real vg/odgi output on every `cargo test`, without
the tools installed; tests of our own writers round-trip in memory instead.
`MANIFEST.tsv` records the source GFA, checksum, tool version and command of
each file. Regenerate them with
[`utils/generate_test_files.sh`](../../../utils/generate_test_files.sh) (vg and
odgi on `PATH`) after changing a fixture or the pinned tool versions.

`fastg/assembly_graph.fastg` is a hand-authored FASTG graph; no tool converts
GFA to FASTG for us.

`gfa/names_mixed.gfa` is hand-authored: it mixes numeric and non-numeric
segment names, including `0` and `007`, and is a regression test for node-id
collisions in formats that need integer ids (VG, ODGI).
