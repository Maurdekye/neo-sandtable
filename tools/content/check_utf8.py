#!/usr/bin/env python3
"""Fail when a tracked text file is not valid UTF-8.

Windows tools default to a legacy code page, so a file can be written as Windows-1252 and still
look fine in the editor that wrote it while every other reader sees invalid bytes (it happened to
two interpretation files). CI runs this over the repo's text files:

    python tools/content/check_utf8.py            # every tracked text file
    python tools/content/check_utf8.py docs data  # only these paths

Exit status is 1 when any file is not UTF-8, 0 otherwise.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TEXT_SUFFIXES = {".md", ".toml", ".csv", ".json", ".txt", ".yml", ".yaml", ".rs", ".ts", ".tsx",
                 ".py", ".sql", ".html", ".css", ".svg"}


def tracked(paths: list[str]) -> list[Path]:
    out = subprocess.run(["git", "ls-files", "-z", "--", *paths], cwd=REPO, check=True,
                         capture_output=True).stdout
    return [REPO / p for p in out.decode("utf-8").split("\0") if p]


def main() -> int:
    bad = []
    for path in tracked(sys.argv[1:]):
        if path.suffix.lower() not in TEXT_SUFFIXES or not path.is_file():
            continue
        try:
            path.read_bytes().decode("utf-8")
        except UnicodeDecodeError as e:
            bad.append(f"{path.relative_to(REPO)}: byte {e.start}: {e.reason}")
    for line in bad:
        print(f"not UTF-8: {line}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
