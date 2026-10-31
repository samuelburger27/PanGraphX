# The ODGI binary format (`.og`)

This document describes the on-disk layout of odgi graphs as PanGraphX reads and
writes it in [`pangraphx-core/src/formats/odgi_format.rs`](../pangraphx-core/src/formats/odgi_format.rs).

odgi has no written specification. A `.og` file is a memory dump of odgi's
in-memory graph (`odgi::graph_t`), so everything below comes from odgi's
serialization code and was checked against files written by `odgi build`
(odgi v0.9.4):

| What | odgi source |
|---|---|
| File header, magic number | `deps/libhandlegraph/src/serializable.cpp` (`Serializable::serialize`) |
| Graph header, path metadata | `src/odgi.cpp` (`graph_t::serialize_members` / `deserialize_members`) |
| Node record, edges, steps | `src/node.hpp`, `src/node.cpp` (`node_t::serialize`, `add_edge`, `add_path_step`, `encode`/`decode`) |
| Packed integer vectors | `deps/DYNAMIC/include/internal/hacked_vector.hpp` (`hacked_vector::serialize`) |
| Handles, ids, ranks | `src/odgi.cpp` (`get_handle`, `get_id`, `create_handle`, `create_edge`, `create_step`, `link_steps`) |

The format has no version field: the magic number is a constant, and the code
that returns it is commented `// TODO update me`. A future odgi release could
change the layout without changing the magic number, which is why PanGraphX
cross-validates against the real `odgi` binary (see [Testing](#testing)).

## Contents

1. [Core concepts](#core-concepts): node ids, ranks, handles, steps
2. [File layout](#file-layout)
3. [Packed vectors](#packed-vectors)
4. [Node records](#node-records): sequence, edges, decoding table, steps
5. [Path metadata](#path-metadata)
6. [Walking a path](#walking-a-path)
7. [Worked example](#worked-example)
8. [What the format does not store](#what-the-format-does-not-store)
9. [Quirks](#quirks)
10. [How PanGraphX reads and writes it](#how-pangraphx-reads-and-writes-it)
11. [Testing](#testing)

## Core concepts

**Node rank and node id.** odgi keeps its nodes in a vector, `node_v`. A node's
position in that vector is its *rank* (0-based). Its *id*, the number users see
(e.g. the GFA segment name), is:

```
id   = rank + 1 + id_increment
rank = id - 1 - id_increment
```

`id_increment` is a header field. It lets odgi shift all ids without rewriting
the nodes, and is normally 0. So ids are contiguous: a graph with ids 1, 2 and 5
has 5 node records, and the records at ranks 2 and 3 (ids 3 and 4) are *deleted
nodes*.

**Handle.** A node in a given orientation, packed into one integer:

```
handle = rank << 1 | is_reverse
```

Handles hold the node **rank**, not its id. In the file they only appear in the
path metadata (the first and last step of each path).

**Edge.** odgi's graphs are bidirected. An edge `(left, right)` joins two
handles, read as "leave `left` from its end, enter `right` at its start". That
is exactly a GFA link `L left.id left.orient right.id right.orient 0M`. The edge
`(left, right)` is the same as its reverse complement `(flip(right), flip(left))`,
and odgi stores only one of the two.

**Path and step.** A path is a named sequence of steps, each step being one
visit of a handle. odgi does not store a path as an array. Each step is stored
on the node it visits, and steps are linked into a doubly-linked list. A step is
identified by a *step handle*:

```
step handle = (handle of the visited node, rank of the step among the steps stored on that node)
```

The second number is the step's *rank on the node*: if a node is visited three
times (by one path or several), its steps have ranks 0, 1 and 2 in the order
they were created.

**Path handle.** Paths are numbered from 1 in creation order. On load, odgi
gives the paths handles `1..=path_count` in the order their metadata appears in
the file.

## File layout

Byte order is the host's (little-endian on every platform odgi supports),
except for the magic number, which is big-endian. `size_t` is 8 bytes. There is
no padding or alignment between fields.

```
offset  size            field
0       4               magic number, big-endian: 1988148666 (0x7680BDBA)
4       8               max_node_id
12      8               min_node_id
20      8               node_count          number of node records, deleted ones included
28      8               edge_count          number of distinct edges
36      8               path_count
44      8               path_handle_next    largest path handle handed out so far
52      8               id_increment
60      ...             node_count × node record      (rank 0, 1, 2, ...)
        ...             path_count × path metadata
```

The header is 60 bytes. Notes on the header fields:

- `max_node_id` and `min_node_id` are the highest and lowest *raw* ids
  (`rank + 1`) of live nodes, without `id_increment`. Both are 0 for an empty
  graph. odgi's `max_node_id()` adds `id_increment` when it reports them.
- `edge_count` counts each edge once, even though it is stored on both of its
  nodes (see [Edges](#edges)).
- `path_handle_next` equals `path_count` unless paths were deleted.
- A file without the magic number is still accepted: libhandlegraph prints a
  warning ("either an old version or in the wrong format"), rewinds 4 bytes and
  reads the header from offset 0.

## Packed vectors

Edges, the decoding table and steps are each stored as a DYNAMIC
`hacked_vector`: a list of unsigned integers that all use the same bit width
and are packed into 64-bit words.

```
size        field
8           word_count
8 × wc      words
8           mask            (1 << width) - 1
8           size            number of values
1           width           bits per value, 1..=64 (0 for a default-constructed empty vector)
1           per_word        values per word = 64 / width (0 when empty)
```

So a packed vector takes `26 + 8 × word_count` bytes. Value `i` is:

```
words[i / per_word] >> ((i % per_word) * width) & mask
```

Values fill each word from the least significant bit up, and the top
`64 mod width` bits of each word are unused. Example: the four 2-bit values
`[2, 0, 3, 1]` pack into one word as `0b01_11_00_10 = 0x72`.

Things a reader must not assume:

- **`word_count` may be larger than needed.** odgi reserves 2 spare words when a
  vector grows, so `odgi build` usually writes 3 words where 1 would do. Only
  `ceil(size / per_word)` words are meaningful.
- **`width` may be wider than needed.** A vector grows its width when a larger
  value is inserted and never narrows it, even after that value is overwritten.
  Readers must use the stored `width`.
- **`mask` is unreliable for width 64.** odgi computes it as `(1 << 64) - 1`,
  which is undefined behaviour in C++. Compute the mask from `width` instead.

## Node records

Each node record is:

```
size        field
8           sequence_length
seq_len     sequence bytes (no terminator)
8           id              raw id = rank + 1; 0 marks a deleted node
...         edges           packed vector, 2 values per edge
...         decoding        packed vector, 1 value per neighbor
...         steps           packed vector, 6 values per step
```

A **deleted node** is written as an empty record: sequence length 0, id 0 and
three empty packed vectors (`26` bytes each, all zero). On load, odgi detects
deleted nodes by `id == 0`. They exist because ids must be contiguous (see
[Core concepts](#core-concepts)) or because a node was removed and not yet
compacted.

The stored `id` is the raw id `rank + 1`, without `id_increment`. It is used as
the base of the delta encoding in the decoding table.

### Edges

Every edge is stored as a 2-value record on each of its two nodes:

```
value 0     other_id        id of the node at the other end (an external id, not delta-encoded)
value 1     packed type     bit 0: other_rev   orientation of the other node's handle in the edge
                            bit 1: on_rev      orientation of this node's handle in the edge
                            bit 2: to_curr     1 if this node is the right end of the edge
```

For an edge `(left, right)`, odgi's `create_edge` writes:

| stored on | other_id | other_rev | on_rev | to_curr |
|---|---|---|---|---|
| left node | `id(right)` | `right.is_reverse` | `left.is_reverse` | 0 |
| right node | `id(left)` | `left.is_reverse` | `right.is_reverse` | 1 |

A **self loop** (left and right on the same node, in any orientations) is
stored only once, as the left record (`to_curr = 0`).

So every edge has exactly one record with `to_curr = 0`. To list each edge once,
read only those records:

```
for each live node n, for each edge record (other, type) of n with to_curr = 0:
    L  id(n)  (on_rev ? - : +)  other  (other_rev ? - : +)  0M
```

`create_edge` adds nothing when `has_edge(left, right)` already holds, and
`has_edge` also matches the reverse complement. So a GFA that lists both
`L 1 + 2 + 0M` and `L 2 - 1 - 0M` produces a single odgi edge, stored in the
direction of the first one.

### Decoding table

The step records below refer to neighboring nodes through a small per-node
dictionary instead of storing their ids. Each entry is a *delta* from this node's
stored (raw) id:

```
delta 0                 the node itself
delta (d << 1) | 1      the node with id  own_id + d   (higher id)
delta (d << 1)          the node with id  own_id - d   (lower id)
```

`node_t::encode(other_id)` searches the table linearly and appends the delta if
it is missing. A step then stores the entry's *index*. So a decoding table holds
every node this node's steps link to, in first-use order. Entries are never
removed, so a table may contain entries that no step uses any more.

Because `create_step` encodes the node itself before a step is linked to its
neighbors, every node with at least one step has delta 0 at index 0.

Decoding a neighbor: `id = own_id ± (delta >> 1)`. The result is an external id
(it includes `id_increment`), the same kind of id stored in edge records.

### Steps

Every visit of the node by a path is a 6-value record. The record's position in
the vector (0, 1, 2, ...) is the step's rank on this node.

```
value 0     path_id         path handle (1-based)
value 1     type            bit 0: is_rev    the node is traversed in reverse
                            bit 1: is_start  first step of its path
                            bit 2: is_end    last step of its path
                            bit 3: is_del    deleted step (rank kept until compaction)
value 2     prev            decoding-table index of the previous step's node
value 3     prev_rank       rank of the previous step on that node
value 4     next            decoding-table index of the next step's node
value 5     next_rank       rank of the next step on that node
```

For the first step of a path (`is_start`), `prev` is the index of the node itself
and `prev_rank` is 0. The same holds for `next` / `next_rank` on the last step.
Readers should check the flags before following a link.

The orientation of a step comes from its own `is_rev` bit. The steps before and
after it are named by node and rank only; their orientations are in their own
records.

## Path metadata

After all node records come `path_count` path records, in path-handle order:

```
size        field
8           length          number of steps
8           first.handle    handle (rank << 1 | is_reverse) of the first step's node
8           first.rank      rank of the first step on that node
8           last.handle     handle of the last step's node
8           last.rank       rank of the last step on that node
8           name_length
name_len    name bytes
```

A path record takes `48 + name_length` bytes. An empty path has length 0 and
first = last = (0, 0).

## Walking a path

To recover a path's steps in order, follow the `next` links from `first`:

```
node_rank, step_rank = first.handle >> 1, first.rank
repeat length times:
    record = steps of node `node_rank`, record number `step_rank`
    emit (id of node_rank, record.is_rev ? reverse : forward)
    if not last:
        next_id   = decode(record.next) using node_rank's decoding table and stored id
        node_rank = next_id - 1 - id_increment
        step_rank = record.next_rank
```

Walking backwards from `last` with `prev` / `prev_rank` gives the same path in
reverse. This requires random access to the step records of every node, so a
reader has to keep all step records in memory before it can produce any path.

## Worked example

This GFA:

```
H	VN:Z:1.0
S	1	A
S	2	CC
S	3	G
L	1	+	2	+	0M
L	2	+	3	-	0M
L	1	+	3	-	0M
P	x	1+,2+,3-	*
P	y	3+,1-	*
```

has three nodes and three edges. Path `x` goes forward through 1 and 2, then
3 in reverse. Path `y` goes 3+ → 1-, which uses the edge `1+ → 3-` in its
reverse-complement direction. `odgi build` turns it into this 660-byte file
(decoded with [`utils/dump_odgi.py`](../utils/dump_odgi.py)):

**Header** (offsets 0–59): magic `76 80 BD BA`, then
`max_node_id = 3, min_node_id = 1, node_count = 3, edge_count = 3, path_count = 2,
path_handle_next = 2, id_increment = 0`.

**Node rank 0** (offset 60): sequence length 1, sequence `A`, id 1.

- *Edges* (offset 77): 3 words `[0x72, 0, 0]`, width 2, size 4 → values
  `[2, 0, 3, 1]`, i.e. two records:
  - `(2, 0b000)`: edge to node 2, both forward, this node is the left end → `L 1 + 2 + 0M`
  - `(3, 0b001)`: edge to node 3, other end reversed → `L 1 + 3 - 0M`
- *Decoding* (offset 127): `[0, 3, 5]` → deltas to ids `[1, 2, 3]`
  (`3 = 1 << 1 | 1` is +1, `5 = 2 << 1 | 1` is +2).
- *Steps* (offset 177): 12 values = 2 records:
  - rank 0: `[1, 2, 0, 0, 1, 0]`: path 1 (`x`), type `start`, prev = itself,
    next = decoding[1] = node 2, step rank 0 there
  - rank 1: `[2, 5, 2, 1, 0, 0]`: path 2 (`y`), type `rev | end`,
    prev = decoding[2] = node 3, step rank 1 there, next = itself

**Node rank 1** (offset 227): sequence `CC`, id 2.

- *Edges*: `[1, 4, 3, 1]`:
  - `(1, 0b100)`: `to_curr` set, so this is node 2's copy of `L 1 + 2 + 0M` (skip it when listing edges)
  - `(3, 0b001)`: `L 2 + 3 - 0M`, stored on its left node
- *Decoding*: `[0, 2, 3]` → ids `[2, 1, 3]` (`2 = 1 << 1` is −1)
- *Steps*: rank 0: `[1, 0, 1, 0, 2, 0]`: path `x`, forward, neither first nor
  last, prev = node 1 rank 0, next = node 3 rank 0.

**Node rank 2** (offset 395): sequence `G`, id 3.

- *Edges*: `[2, 6, 1, 6]`: both records have `to_curr | on_rev` (`0b110`), so
  they are node 3's copies of `L 2 + 3 - 0M` and `L 1 + 3 - 0M`. Node 3 is the
  reversed right end of both.
- *Decoding*: `[0, 2, 4]` → ids `[3, 2, 1]`
- *Steps*:
  - rank 0: `[1, 5, 1, 0, 0, 0]`: path `x`, `rev | end`, prev = node 2 rank 0
  - rank 1: `[2, 2, 0, 0, 2, 1]`: path `y`, `start`, forward, next = node 1 rank 1

**Paths** (offsets 562 and 611):

| name | length | first (handle, rank) | last (handle, rank) |
|---|---|---|---|
| `x` | 3 | (0, 0) = node rank 0 forward, its step 0 | (5, 0) = node rank 2 reverse, its step 0 |
| `y` | 2 | (4, 1) = node rank 2 forward, its step 1 | (1, 1) = node rank 0 reverse, its step 1 |

Walking `y`: start at node rank 2, step 1 (`3+`). Its next link is decoding[2] =
id 1, step rank 1. Node rank 0's step 1 is reversed and is the end, giving
`3+, 1-`.

## What the format does not store

- **Overlaps.** Edges and paths have no overlap or CIGAR fields. `odgi build`
  ignores GFA overlaps.
- **Node names.** Nodes only have integer ids, so `odgi build` rejects GFA files
  whose segment names are not positive integers.
- **Path circularity.** odgi keeps an `is_circular` flag in memory but does not
  write it.
- **Header lines, tags and other GFA metadata.**

## Quirks

- **Padding words and wide vectors**, see [Packed vectors](#packed-vectors).
  Two valid files of the same graph can therefore differ byte for byte.
- **Stale entries.** Decoding tables can contain unused entries, and step
  vectors can contain deleted steps (`is_del`) that still take up a rank. odgi's
  editing operations can leave both behind; `odgi build` output has none.
- **Path ids in step records.** On load, odgi reassigns path handles as
  `1..=path_count` in file order, but it does not rewrite the `path_id` inside
  step records. If paths were deleted before the file was saved, the two may
  disagree. Walking a path does not need `path_id`.
- **Edge direction.** odgi keeps an edge in the direction it was first created:
  `L 2 - 1 - 0M` is written back by `odgi view` as `L 2 - 1 - 0M`, not as the
  equivalent `L 1 + 2 + 0M`. If a GFA lists both directions, the first one wins.

## How PanGraphX reads and writes it

**Reading** (`ODGICodec::parse`):

1. Check the magic number; if it is missing, warn and rewind like odgi does.
2. Read the node records in 16 MiB blocks. Record boundaries (and each record's
   step count) are found sequentially from the length fields alone; the records
   of a block are then decoded in parallel, each task writing its steps straight
   into its part of the final step table. A live node becomes a `Node`; a
   deleted node is skipped.
3. Keep only edge records with `to_curr = 0`, giving each edge once.
4. Keep an 8-byte link (next node rank, next step rank, `is_rev`; an end marker
   instead of a node for a path's last step) for every step record, grouped by
   node. The decoding table is resolved here and then dropped.
5. Read the path metadata and walk all paths in parallel. A path is a linked
   list scattered over the node records, so every step is a dependent cache
   miss; each thread therefore advances up to 16 paths in lockstep, letting
   their misses overlap.
6. Nodes are named by their odgi id (`rank + 1 + id_increment`). Building that
   map is latency-bound too, so each block's names are inserted while the next
   block is decoded, and the last block's while the paths are walked.

Every length, index, rank and id is bounds-checked; corrupt input returns a
`PanGraphXError::Parse` rather than panicking or making huge allocations (the
node count, the step table and every reservation are bounded by the input
size). Step ranks are packed into 31 bits, so a node can have at most 2^31
steps; the writer enforces the same limit.

**Writing** (`ODGICodec::serialize`):

1. *Numbering.* If every node name is a canonical positive integer and the
   largest one is at most `2n + 1024`, the names are used as odgi ids, and gaps
   become deleted-node records. Otherwise nodes get ids `1..=n` in node order and
   a warning is logged. `id_increment` is always 0.
2. *Edges.* Each edge is laid out on both endpoints, self loops once, in input
   order. Duplicate edges and reverse complements are then dropped (as odgi's
   `has_edge` does) by de-duplicating each node's records in parallel: every
   copy of an edge is on both of its endpoints, so this keeps the first copy on
   both sides.
3. *Steps.* An inverted index lists the steps on every node in path order; it is
   built in parallel over node ranges and stores 4-byte step indices when the
   paths have fewer than 2^32 steps. A step's rank on its node is its position
   in that list, found by an exponential search that resumes where the node's
   previous lookup ended, so no per-step rank is stored. Each node record gets
   its step records, with prev/next links pointing to (node, rank). The decoding
   table starts with the node itself and then lists neighbors in first-use
   order, as in odgi.
4. The three indexes are built concurrently. Node records are encoded in
   parallel chunks and streamed out in order, the next window being encoded
   while the current one is written; packed vectors use the smallest width and
   no spare words.

For the worked example above, every value PanGraphX writes matches `odgi
build`; only the spare words and some vector widths differ. On the benchmark
graphs this makes PanGraphX files about 4–10 % smaller (ecoli50: 387 vs 428 MB,
HPRC chr22: 679 vs 710 MB).

## Testing

- `cargo test -p pangraphx-core odgi` runs the unit tests.
- [`pangraphx-core/tests/test_odgi.rs`](../pangraphx-core/tests/test_odgi.rs)
  round-trips every GFA fixture through ODGI, and
  [`test_reference_files.rs`](../pangraphx-core/tests/test_reference_files.rs)
  parses the committed `test_files/og/*.og` files written by `odgi build`.
- [`cross_validation.rs`](../pangraphx-core/tests/cross_validation.rs) checks
  PanGraphX output with `odgi validate` and `odgi view -g`, and parses fresh
  `odgi build` output, when `odgi` is on `PATH` or in `PANGRAPHX_ODGI`. Run it
  after every change to the codec.
- `utils/dump_odgi.py graph.og` prints every field of a file, which helps when
  comparing PanGraphX output with odgi's.
