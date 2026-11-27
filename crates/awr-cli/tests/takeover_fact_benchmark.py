#!/usr/bin/env python3
"""Generate a synthetic Git fast-import stream for takeover fact benchmarks."""

import argparse
import sys


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--commits", type=int, default=100_000)
    args = parser.parse_args()
    if args.commits < 1:
        parser.error("--commits must be positive")

    out = sys.stdout.buffer
    for mark, content in ((1, b"zero\n"), (2, b"one\n")):
        out.write(b"blob\n")
        out.write(f"mark :{mark}\n".encode())
        out.write(f"data {len(content)}\n".encode())
        out.write(content)

    for index in range(args.commits):
        title = f"synthetic-{index:06d}\n".encode()
        out.write(b"commit refs/heads/main\n")
        out.write(b"committer Fixture <fixture@example.invalid> 1700000000 +0000\n")
        out.write(f"data {len(title)}\n".encode())
        out.write(title)
        out.write(f"M 100644 :{index % 2 + 1} file.txt\n\n".encode())
    out.write(b"done\n")


if __name__ == "__main__":
    main()
