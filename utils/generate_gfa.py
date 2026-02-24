#!/usr/bin/python3
import random
import argparse


def random_seq(length):
    return ''.join(random.choices("ACGT", k=length))


def generate_gfa(filename, segment_count=1000, avg_len=500, link_prob=0.4, num_paths=2):
    with open(filename, "w") as f:
        f.write("H\tVN:Z:1.0\n")  # GFA header
        # Segments
        for i in range(1, segment_count + 1):
            seq = random_seq(random.randint(avg_len // 2, avg_len * 3 // 2))
            f.write(f"S\tS{i}\t{seq}\n")

        # Links (edges)
        for i in range(1, segment_count):
            if random.random() < link_prob:
                j = random.randint(i + 1, segment_count)
                ori1 = random.choice(["+", "-"])
                ori2 = random.choice(["+", "-"])
                f.write(f"L\tS{i}\t{ori1}\tS{j}\t{ori2}\t0M\n")

        # Paths
        for p in range(num_paths):
            f.write(f"P\tpath{p+1}\t")
            path = []
            for i in range(1, segment_count + 1):
                if random.random() < 0.4:  # 40% chance to include the segment in the path
                    path.append(f"S{i}+")
            f.write(",".join(path) + "\t*\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="Generate synthetic GFA file for testing.")
    parser.add_argument(
        "--file", default="test_files/random.gfa", help="Output GFA filename")
    parser.add_argument("--segments", type=int, default=100,
                        help="Number of segments (nodes)")
    parser.add_argument("--avg-len", type=int, default=50,
                        help="Average sequence length")
    parser.add_argument("--link-prob", type=float, default=0.8,
                        help="Probability of link creation between segments")

    parser.add_argument("--seed", type=int, default=69,
                        help="Random seed for reproducibility")
    parser.add_argument("--num-paths", type=int, default=2,
                        help="Number of paths to include in the GFA")
    args = parser.parse_args()
    random.seed(args.seed)
    generate_gfa(args.file, args.segments, args.avg_len,
                 args.link_prob, args.num_paths)
    print(f"Generated {args.file} with {args.segments} segments.")
