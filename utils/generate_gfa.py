#!/usr/bin/python3
"""Generate a synthetic, valid GFA 1.0 pangenome graph for testing.

Every path is a walk over the segments in increasing order, visiting each
segment with probability `--visit-prob` and occasionally in reverse
(`--inversion-prob`). Every segment lies on at least one path, and every pair of
consecutive path steps is joined by an `L` line, so paths respect the graph
topology (`odgi validate` accepts the output). By default every link lies on a
path, as in a graph built from haplotypes, which GBZ (a path-based format) can
represent exactly; `--extra-links` adds links that no path walks.

Fixtures:
  utils/generate_gfa.py --file random.gfa --segments 100 --avg-len 50 --num-paths 2
  utils/generate_gfa.py --file random_medium.gfa
"""
import argparse
import random


def random_seq(length):
    return ''.join(random.choices("ACGT", k=length))


def flip(orientation):
    return "-" if orientation == "+" else "+"


def canonical(link):
    """A link and its reverse complement are the same link."""
    a, oa, b, ob = link
    return min((a, oa, b, ob), (b, flip(ob), a, flip(oa)))


def generate_paths(segment_count, num_paths, visit_prob, inversion_prob):
    """Returns each path as a list of (segment, orientation), in segment order."""
    visits = [
        [i for i in range(1, segment_count + 1) if random.random() < visit_prob]
        for _ in range(num_paths)
    ]
    # Give every segment that no path visits to a random path.
    visited = set().union(*visits)
    for i in range(1, segment_count + 1):
        if i not in visited:
            visits[random.randrange(num_paths)].append(i)
    return [
        [(i, "-" if random.random() < inversion_prob else "+") for i in sorted(path)]
        for path in visits
    ]


def generate_gfa(filename, segment_count=1000, avg_len=500, num_paths=2,
                 visit_prob=0.4, inversion_prob=0.02, extra_links=0):
    if num_paths < 1:
        raise ValueError("at least one path is needed to place every segment")
    paths = generate_paths(segment_count, num_paths, visit_prob, inversion_prob)

    # Links joining consecutive steps; first-seen direction is kept.
    links = {}
    for path in paths:
        for (a, oa), (b, ob) in zip(path, path[1:]):
            links.setdefault(canonical((a, oa, b, ob)), (a, oa, b, ob))
    added = 0
    while added < extra_links and segment_count > 1:
        a = random.randint(1, segment_count - 1)
        link = (a, random.choice("+-"), random.randint(a + 1, segment_count), random.choice("+-"))
        if canonical(link) not in links:
            links[canonical(link)] = link
            added += 1

    with open(filename, "w") as f:
        f.write("H\tVN:Z:1.0\n")
        for i in range(1, segment_count + 1):
            seq = random_seq(random.randint(avg_len // 2, avg_len * 3 // 2))
            f.write(f"S\tS{i}\t{seq}\n")
        for a, oa, b, ob in sorted(links.values()):
            f.write(f"L\tS{a}\t{oa}\tS{b}\t{ob}\t0M\n")
        for p, path in enumerate(paths, start=1):
            steps = ",".join(f"S{i}{o}" for i, o in path)
            f.write(f"P\tpath{p}\t{steps}\t*\n")
    return len(links)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--file", default="random_medium.gfa", help="Output GFA filename")
    parser.add_argument("--segments", type=int, default=50_000,
                        help="Number of segments (nodes)")
    parser.add_argument("--avg-len", type=int, default=1_000,
                        help="Average sequence length")
    parser.add_argument("--num-paths", type=int, default=25,
                        help="Number of paths to include in the GFA")
    parser.add_argument("--visit-prob", type=float, default=0.4,
                        help="Probability that a path visits a given segment")
    parser.add_argument("--inversion-prob", type=float, default=0.02,
                        help="Probability that a path step is reversed")
    parser.add_argument("--extra-links", type=int, default=0,
                        help="Number of additional links not walked by any path")
    parser.add_argument("--seed", type=int, default=69,
                        help="Random seed for reproducibility")
    args = parser.parse_args()
    random.seed(args.seed)
    link_count = generate_gfa(args.file, args.segments, args.avg_len, args.num_paths,
                              args.visit_prob, args.inversion_prob, args.extra_links)
    print(f"Generated {args.file} with {args.segments} segments, "
          f"{link_count} links and {args.num_paths} paths.")
