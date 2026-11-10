#!/usr/bin/env python3
"""Emit a synthetic report illustrating AWR's mutation report contract."""

import argparse
import json
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-sha", help="Source commit; defaults to the current Git HEAD")
    args = parser.parse_args()
    source_sha = args.source_sha or subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True
    ).strip()
    print(json.dumps({
        "version": 1,
        "source_sha": source_sha,
        "tool": "synthetic-format-example 1",
        "mutants": [
            {"id": "save-result", "file": "src/save.rs", "line": 42,
             "status": "killed", "killed_by": ["keeps-input"]},
            {"id": "guarded-branch", "file": "src/save.rs", "line": 57,
             "status": "equivalent", "note": "An earlier guard excludes this branch"},
        ],
    }))


if __name__ == "__main__":
    main()
