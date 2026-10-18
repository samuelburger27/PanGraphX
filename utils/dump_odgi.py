#!/usr/bin/env python3
"""Print every field of an odgi `.og` file, for debugging the ODGI codec.

Usage: utils/dump_odgi.py graph.og

The layout is described in docs/odgi_format.md. Packed vectors are printed as
their raw words and as the decoded values; offsets are byte positions in the file.
"""

import struct
import sys

ODGI_MAGIC = 1988148666
HEADER_FIELDS = (
    "max_node_id",
    "min_node_id",
    "node_count",
    "edge_count",
    "path_count",
    "path_handle_next",
    "id_increment",
)


class Reader:
    def __init__(self, data):
        self.data = data
        self.pos = 0

    def u64(self):
        (value,) = struct.unpack_from("<Q", self.data, self.pos)
        self.pos += 8
        return value

    def u8(self):
        value = self.data[self.pos]
        self.pos += 1
        return value

    def bytes(self, n):
        value = self.data[self.pos : self.pos + n]
        if len(value) != n:
            raise ValueError(f"truncated file at offset {self.pos}")
        self.pos += n
        return value

    def packed(self):
        """Reads a DYNAMIC hacked_vector and returns its decoded values."""
        start = self.pos
        words = [self.u64() for _ in range(self.u64())]
        mask, size, width, per_word = self.u64(), self.u64(), self.u8(), self.u8()
        values = []
        if size:
            value_mask = (1 << width) - 1
            values = [
                (words[i // per_word] >> ((i % per_word) * width)) & value_mask
                for i in range(size)
            ]
        raw = " ".join(f"{w:#x}" for w in words)
        print(
            f"    @{start}: {len(words)} words [{raw}] mask={mask:#x} "
            f"size={size} width={width} per_word={per_word}"
        )
        return values


def main(path):
    r = Reader(open(path, "rb").read())
    magic = struct.unpack_from(">I", r.data, 0)[0]
    if magic == ODGI_MAGIC:
        r.pos = 4
        print(f"@0 magic {magic}")
    else:
        print("no magic number; reading the header from offset 0")
    header = dict(zip(HEADER_FIELDS, (r.u64() for _ in HEADER_FIELDS)))
    print(f"@{r.pos - 56} header {header}")

    for rank in range(header["node_count"]):
        start = r.pos
        sequence = r.bytes(r.u64())
        stored_id = r.u64()
        print(f"\nnode rank {rank} @{start}: id={stored_id} sequence={sequence.decode(errors='replace')}")
        if stored_id == 0:
            print("  (deleted node)")
        print("  edges:")
        edges = r.packed()
        for i in range(0, len(edges), 2):
            other, packed = edges[i], edges[i + 1]
            print(
                f"      other={other} other_rev={packed & 1} on_rev={packed >> 1 & 1} "
                f"to_curr={packed >> 2 & 1}"
            )
        print("  decoding:")
        decoding = r.packed()
        # Deltas are relative to the stored id and decode to neighbor odgi ids.
        own = stored_id

        def neighbor(index):
            delta = decoding[index]
            return own + (delta >> 1) if delta & 1 else own - (delta >> 1)

        print(f"      ids={[neighbor(i) for i in range(len(decoding))]}")
        print("  steps:")
        steps = r.packed()
        for i in range(0, len(steps), 6):
            path_id, kind, prev, prev_rank, nxt, next_rank = steps[i : i + 6]
            flags = [n for bit, n in ((1, "rev"), (2, "start"), (4, "end"), (8, "del")) if kind & bit]
            print(
                f"      rank {i // 6}: path={path_id} type={kind} {flags} "
                f"prev=id {neighbor(prev)} rank {prev_rank} next=id {neighbor(nxt)} rank {next_rank}"
            )

    print()
    for _ in range(header["path_count"]):
        start = r.pos
        length = r.u64()
        first = (r.u64(), r.u64())
        last = (r.u64(), r.u64())
        name = r.bytes(r.u64()).decode(errors="replace")

        def step(handle_rank):
            handle, rank = handle_rank
            return f"(node rank {handle >> 1}{'-' if handle & 1 else '+'}, step rank {rank})"

        print(f"path @{start}: name={name} length={length} first={step(first)} last={step(last)}")
    print(f"\nend @{r.pos} of {len(r.data)} bytes")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
