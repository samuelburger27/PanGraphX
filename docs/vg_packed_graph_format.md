# vg's PackedGraph and HashGraph formats (`.pg`, `.hg`, `.vg`)

This document describes the on-disk layout of the graph formats current vg
writes, as PanGraphX reads them in
[`packed_graph_format.rs`](../pangraphx-core/src/formats/packed_graph_format.rs)
(PackedGraph, read and written) and
[`hash_graph_format.rs`](../pangraphx-core/src/formats/hash_graph_format.rs)
(HashGraph, read only).

vg's original Protobuf format (`vg convert -v`) is deprecated for output: since
vg switched to libbdsg graphs, `vg convert`, `vg construct` and the other tools
write **PackedGraph** by default, usually under a `.vg` name. vg tells the
formats apart by their first bytes, not by the file name, and so does
PanGraphX: [`VGCodec`](../pangraphx-core/src/formats/vg_format.rs) parses a
`.vg` file as Protobuf, PackedGraph or HashGraph, whichever it holds.

Like odgi, libbdsg has no written specification. A file is a dump of the
in-memory graph, so everything below comes from the serialization code (libbdsg
as vendored by vg v1.73) and was checked against files written by
`vg convert`:

| What | Source |
|---|---|
| Magic numbers, dispatch on them | `vg/src/io/register_loader_saver_{packed,hash}_graph.cpp`, libvgio `registry.hpp` |
| PackedGraph members | `libbdsg/bdsg/include/bdsg/internal/base_packed_graph.hpp` (`serialize_members`, `create_edge`, `append_step`, `create_path_handle`) |
| Packed, paged, deque, robust vectors | `libbdsg/bdsg/include/bdsg/internal/packed_structs.hpp` |
| Integer vectors | `sdsl-lite/include/sdsl/int_vector.hpp` (`int_vector<0>::serialize`) |
| HashGraph members | `libbdsg/bdsg/src/hash_graph.cpp` (`serialize_members`, `node_t::serialize`, `path_t::serialize`, `create_edge`) |

## Contents

1. [Telling the formats apart](#telling-the-formats-apart)
2. [Handles](#handles)
3. [PackedGraph](#packedgraph): vectors, file layout, nodes, edges, paths
4. [Worked example](#worked-example)
5. [HashGraph](#hashgraph)
6. [What the formats do not store](#what-the-formats-do-not-store)
7. [How PanGraphX reads and writes them](#how-pangraphx-reads-and-writes-them)
8. [Testing](#testing)

## Telling the formats apart

Both libbdsg formats are written *bare*, starting with a big-endian `u32` magic
number:

| First 4 bytes | Magic | Format |
|---|---|---|
| `b7 9e f7 5d` | 3080648541 | PackedGraph |
| `28 4d 4f 38` | 676155192 | HashGraph |
| `28 11 5c 8f` | 672226447 | MappedPackedGraph (memory-mapped variant; not supported) |
| anything else | | Protobuf stream in VG framing (groups of varint-length messages, usually tagged `VG`) |

vg's loader also accepts a bare format wrapped in VG framing under the type tag
`PackedGraph` or `HashGraph`: the messages after the tag, concatenated, are
the bare file. vg 1.73 does not write that form, but PanGraphX reads it.

## Handles

As everywhere in libhandlegraph, an oriented node is a *handle*
`id << 1 | is_reverse`, and `flip(h) = h ^ 1`. Node ids are positive.

## PackedGraph

### Vectors

Every integer is little-endian, and every vector is built on sdsl's
`int_vector<>`:

```text
int_vector  u64 bit length, u8 width, ⌈bit length / 64⌉ u64 words
packed      u64 length, int_vector
paged       u64 length, u64 page size, packed anchors, then one packed page per anchor
deque       u64 begin, u64 length, packed ring buffer
robust      packed first page (≤ 256 values), paged rest (page size 256)
string      u64 length, bytes
```

- **int_vector**: `bit length / width` values of `width` bits (1..=64), packed
  back to back from the least significant bit; a value may span two words.
- **packed** (`PackedVector`): the first `length` values of the int vector,
  which may hold more (spare capacity).
- **paged** (`PagedVector`): values in pages of a fixed size (256 or 1024, also
  checked on load). Each page stores values relative to its *anchor*, the first
  non-zero value written to the page, which keeps the widths small for slowly
  increasing values. A stored value `v` decodes as

  ```text
  0                       if v = 0       (0 stays 0: it ends linked lists)
  anchor − v/5            if 5 divides v
  anchor + v − v/5 − 1    otherwise
  ```

  so the differences 0, 1, 2, 3, −1, 4, 5, 6, 7, −2, … are stored as 1, 2, 3,
  …. Every page holds the full page size; the last one is padded with zeros.
- **deque** (`PackedDeque`): a ring buffer; value `i` is at
  `begin + i` if that is below the buffer length, else `i − (length − begin)`.
- **robust** (`RobustPagedVector`): the first 256 values unpaged, the rest
  paged.

### File layout

Node records, edge records, memberships and steps are all numbered from 1,
so that 0 ends a linked list. `*` marks paged vectors with 1024-value pages.

```text
u32 BE   magic 3080648541
i64      max_id
i64      min_id                    (i64::MAX in an empty graph)
paged    graph_iv                  per node record: start-side edge list, end-side edge list
paged    seq_start_iv              per record: offset of its sequence in seq_iv
packed   seq_length_iv             per record: sequence length
paged*   edge_lists_iv             per edge record: target handle, next edge record
deque    nid_to_graph_iv           id − min_id → record (0: no node with that id)
packed   seq_iv                    bases: A C G T N = 0 1 2 3 4
paged    path_membership_node_iv   per record: first membership of the node
paged*   path_membership_id_iv     per membership: path index (0-based)
paged    path_membership_offset_iv per membership: the step on that path
paged    path_membership_next_iv   per membership: next membership of the same node
string   inverse_char_assignment   the distinct bytes of all path names
packed   path_names_iv             all path names, as indices into those bytes
paged    path_name_start_iv        per path: start of its name in path_names_iv
packed   path_name_length_iv       per path: name length
packed   path_is_deleted_iv        per path: 1 if deleted
packed   path_is_circular_iv       per path: 1 if circular
paged*   path_head_iv              per path: first step
paged    path_tail_iv              per path: last step
packed   path_deleted_steps_iv     per path: number of deleted steps
per path:
  robust links_iv                  per step: previous step, next step
  robust steps_iv                  per step: handle
u64 ×6   deleted node records, deleted edge records, deleted memberships,
         deleted bases, reversing self-edges, deleted reversing self-edges
```

### Nodes

Node `id` exists if `nid_to_graph_iv[id − min_id] = r ≠ 0`. Its sequence is
`seq_iv[seq_start_iv[r − 1] ..][.. seq_length_iv[r − 1]]`. Records keep the
order the nodes were created in; the id table makes the id range dense, so a
graph with ids 1 and 10⁹ stores a billion-entry table.

### Edges

`graph_iv[2(r − 1)]` and `graph_iv[2(r − 1) + 1]` are the heads of the edge
lists of the node's start and end side. The end-side list holds the handles
reached leaving the node forward, the start-side list those reached leaving
it in reverse. Edge record `e` is `edge_lists_iv[2(e − 1)]` (target handle)
and `edge_lists_iv[2(e − 1) + 1]` (next record).

`create_edge(a, b)` prepends `b` to the list of the side `a` leaves (its end if
`a` is forward), and `flip(a)` to the list of the side `b` enters (its start if
`b` is forward). Every edge is therefore listed twice, once as `a → b` and once
as its reverse complement `flip(b) → flip(a)`, except a *reversing self-edge*
`a+ → a-` or `a- → a+`, which leaves and enters the same side and is listed
once (and counted in the trailer).

### Paths

Path `i` is named by `path_names_iv[path_name_start_iv[i] ..][.. length]`,
each value an index into `inverse_char_assignment`. Its steps are a doubly
linked list in `links_iv` / `steps_iv`, starting at `path_head_iv[i]`: step `s`
visits `steps_iv[s − 1]` and continues with `links_iv[2(s − 1) + 1]`. In a
circular path the tail links back to the head.

The *memberships* index the steps by node, for `steps_of_handle`: a node's list
starts at `path_membership_node_iv[r − 1]`, and membership `m` names path
`path_membership_id_iv[m − 1]`, step `path_membership_offset_iv[m − 1]` and the
next membership `path_membership_next_iv[m − 1]`. `append_step` prepends to the
list, so it runs from the latest visit back.

### Deleted records

Editing a graph in vg unlinks records instead of moving data: a deleted node
keeps its record but leaves the id table, edges and steps are cut out of their
lists, a deleted path keeps its vectors and is marked in `path_is_deleted_iv`.
vg compacts a vector only when at least 20 % of its records are orphaned
(`defrag_factor`), so files written after small edits (e.g. `vg paths -d`
dropping one path of many) contain unlinked records. A reader must follow the
lists and the id table rather than read the vectors front to back.

## Worked example

`vg convert --gfa-in --packed-out` of

```text
S  1  ACG          L  1 + 2 + 0M
S  2  T            L  1 + 3 - 0M
S  3  GA           L  2 + 3 - 0M
P  x  1+,2+,3-
```

is 2173 bytes. Decoded (the raw page values of the paged vectors in brackets):

```text
   0       magic b7 9e f7 5d
   4..  20 max_id 3, min_id 1
  20.. 174 graph_iv        0 3 | 2 5 | 0 6          [anchor 1: 0 3 2 6 0 7]
 174.. 296 seq_start_iv    0 3 4                    [anchor 3: 0 1 2]
 296.. 321 seq_length_iv   3 1 2
 321.. 891 edge_lists_iv   e1 (4, 0)  e2 (3, 0)  e3 (7, 1)  e4 (3, 0)  e5 (7, 0)  e6 (5, 4)
 891.. 932 nid_to_graph_iv begin 0: 1 2 3
 932.. 957 seq_iv          0 1 2 3 2 0              (ACG T GA)
 957..1477 memberships     node heads 1 2 3; path 0 0 0; step 1 2 3; next 0 0 0
1477..1486 alphabet        "x"
1486..2009 path metadata   name [0] at 0, length 1, not deleted, not circular, head 1, tail 3
2009..2067 x.links_iv      (0, 2) (1, 3) (2, 0)
2067..2125 x.steps_iv      2 4 7                    (1+ 2+ 3-)
2125..2173 trailer         0 0 0 0 0 0
```

Node 1 (record 1) has end list e3 → e1: `1+ → 3-` (handle 7) and `1+ → 2+`
(handle 4). Node 2 has start list e2 (`2- → 1-`, the mirror of `1+ → 2+`) and
end list e5 (`2+ → 3-`). Node 3 has an empty start list and end list e6 → e4:
`3+ → 2-` and `3+ → 1-`, the mirrors of the two edges entering `3-`. Each edge
appears twice, as described above.

With anchor 4, the edge list values 4 0 3 0 7 1 3 0 7 0 5 4 are stored as
1 0 5 0 4 15 5 0 4 0 2 1. A whole 1024-value page is written for the 12
values, so small files are mostly padding.

## HashGraph

`vg convert --hash-out` writes HashGraph, whose layout is simpler and, unlike
everything above, **big-endian**:

```text
u32      magic 676155192
i64 ×3   max_id, min_id, next_path_id
u64      node count, then per node:
           i64 id, u64 length, sequence bytes,
           u64 n, n × i64 left edge, u64 m, m × i64 right edge
u64      path count, then per path:
           u8 is_circular, i64 path id, u64 name length, name bytes,
           u64 step count, step count × i64 handle
```

Nodes and paths come in hash-table order. The right edges of a node are the
handles reached leaving it forward, the left edges those reached leaving it in
reverse, and every edge is listed on both sides except a reversing self-edge,
as in PackedGraph. Sequences are stored as written.

## What the formats do not store

- **Overlaps**, on edges or between path steps.
- **Bases other than ACGTN** (PackedGraph): `encode_nucleotide` maps every
  other byte, including lowercase `n`, to `N`, and lowercase `acgt` to
  uppercase. HashGraph keeps the bytes.
- **Node names**: nodes have integer ids only.
- **Repeated edges**: `create_edge` ignores an edge (or its reverse complement)
  that already exists.

Path metadata (sample, haplotype, locus) lives in the path name, in vg's
PanSN-style encoding, and passes through PanGraphX unchanged.

## How PanGraphX reads and writes them

**Reading** (`read_packed_graph`, `read_hash_graph`) loads the vectors in their
packed form and decodes values on access, so memory stays close to the file
size plus the graph model. Nodes are numbered in id order and named by their
id. Edge lists are walked per node, in parallel, keeping of every edge the copy
whose `(from, to)` handle pair is smaller than its reverse complement's
(`is_canonical_copy`); paths are walked from their heads. Every index read from
the file is bounds-checked, and list walks are bounded by the number of
records, so a corrupt or looping file is a parse error. Circular paths are read
as linear, with a warning.

**Writing** (`PackedGraphCodec`) produces the graph libbdsg would hold after
creating the nodes, edges and paths in order, with no deleted records. The
files are equivalent to vg's, not byte-identical: a page anchor is the first
non-zero *final* value of the page, while libbdsg keeps the first value ever
written to it.

- Node ids follow the ODGI rule (`id_map::Numbering`): numeric names are kept
  when they are distinct positive integers within a dense range (at most
  `2n + 1024`), otherwise nodes are numbered `1..=n`.
- Edges are deduplicated up to reverse complement and listed as `create_edge`
  lists them, with reversing self-edges counted in the trailer.
- Paths get their memberships in `append_step` order. Non-ACGTN bases are
  written as `N` with one warning giving the count.
- Every vector is streamed from an iterator in two passes (widths and anchors
  first, then the packed words), without materializing it; step lists are
  encoded in parallel, a window of paths at a time.

On HPRC chr22 (1.28 M nodes, 1.78 M edges, 512 paths), on an 8-core laptop:
GFA → PackedGraph takes 2.8 s and 875 MB peak (`vg convert -g -p`: 46 s,
1.7 GB) and gives a 639 MB file (vg: 643 MB); reading it back takes 1.1 s.

## Testing

- Unit tests in both modules: vector encodings (values spanning words, anchor
  differences, paged and robust vectors), a hand-built file with deleted
  records and circular and deleted paths, round trips, id numbering, the
  ACGTN restriction, and truncated or corrupt input.
- `tests/test_packed_graph.rs`: round trips of every GFA fixture.
- `tests/test_files/pg/` and `hg/`: files written by `vg convert` and checked
  tool-free by `tests/test_reference_files.rs`, also through the VG parser.
- `tests/cross_validation.rs` (needs `vg`): `vg validate` and `vg view` of our
  PackedGraph files; our parse of `vg convert` output in PackedGraph,
  HashGraph and default form; and our parse of a PackedGraph edited with
  `vg paths -d`, which keeps unlinked records.
