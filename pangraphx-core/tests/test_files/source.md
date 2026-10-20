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

Only the `gfa/` files are checked into the repository. The `vg/`, `gbz/`,
`fastg/` and `og/` fixtures are generated from the GFA files with
[`utils/generate_test_files.sh`](../../../utils/generate_test_files.sh) (or the
external `vg`/`odgi` tools for cross-validation) and are gitignored. The
hand-authored `fastg/assembly_graph.fastg` is the one exception and stays
in-repo, as does `odgi/tiny.og`: a small file written by `odgi build` (taken
from the MIT-licensed [odgi-ffi](https://github.com/caelrith/odgi-ffi) test data)
used to check that the ODGI parser reads real odgi output.

`gfa/names_mixed.gfa` is hand-authored: it mixes numeric and non-numeric
segment names, including `0` and `007`, and is a regression test for node-id
collisions in formats that need integer ids (VG, ODGI).
