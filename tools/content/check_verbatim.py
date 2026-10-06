#!/usr/bin/env python3
"""Find rulebook prose copied verbatim into the repo.

The content rules (CONTRIBUTING.md section 2) forbid committing verbatim rules prose: every
summary, note, footnote and interpretation must be in our own words. This tool makes that bar
measurable. It compares every text file under the given repo paths (default: data/ and docs/)
against the local rules text in $CNA_SOURCES/rules-2021/*.txt and reports each run of
--min-words or more consecutive words that also occurs, in order, in a rulebook.

A run whose source passage is made of capitalised words (an official phase, segment, table, unit
or place name) is classed as a NAME and is not reported unless --include-names is given; names
and short titles are allowed by the content rules.

The sources are local only and never enter the repo, so this check cannot run in CI. Run it
before pushing anything that contains prose:

    python tools/content/check_verbatim.py                      # all of data/ and docs/
    python tools/content/check_verbatim.py data/rules/airlog    # one area

Exit status is 1 when a prose run is found, 0 otherwise.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TEXT_SUFFIXES = {".toml", ".md", ".csv", ".json", ".txt", ".yaml", ".yml"}
TOKEN = re.compile(r"[A-Za-z0-9]+")
FUNCTION_WORDS = set(
    """a an the of to in on at by for from with without into onto out and or nor but not no any
    each every all some that this these those which who whom whose it its is are was were be been
    being may must can cannot shall will would should could might as if than then so only also per
    he his him they their them there here when where while during before after until unless
    s""".split()
)


def tokens(text: str):
    """Yield (lowercase word, original word, char offset) for every word in text."""
    for m in TOKEN.finditer(text):
        yield m.group(0).lower(), m.group(0), m.start()


def load_sources(sources: Path) -> tuple[list[str], list[str]]:
    lower: list[str] = []
    orig: list[str] = []
    files = sorted(p for p in (sources / "rules-2021").glob("*.txt") if ".layout." not in p.name)
    if not files:
        raise SystemExit(f"no rules text found under {sources / 'rules-2021'}")
    for p in files:
        for lo, og, _ in tokens(p.read_text(encoding="utf-8", errors="replace")):
            lower.append(lo)
            orig.append(og)
        lower.append("\x00")  # a sentinel, so no run spans two books
        orig.append("\x00")
    return lower, orig


def is_name(words: list[str]) -> bool:
    content = [w for w in words if w.lower() not in FUNCTION_WORDS and not w.isdigit()]
    if not content:
        return False
    capitalised = sum(1 for w in content if w[0].isupper())
    return capitalised / len(content) >= 0.8


def scan_file(path: Path, n: int, grams: dict, src_orig: list[str]):
    text = path.read_text(encoding="utf-8", errors="replace")
    toks = list(tokens(text))
    words = [t[0] for t in toks]
    i = 0
    while i <= len(words) - n:
        start = grams.get(tuple(words[i : i + n]))
        if start is None:
            i += 1
            continue
        j = i + n
        while j < len(words) and tuple(words[j - n + 1 : j + 1]) in grams:
            j += 1
        length = j - i
        line = text.count("\n", 0, toks[i][2]) + 1
        yield {
            "file": path.relative_to(REPO).as_posix(),
            "line": line,
            "words": length,
            "name": is_name(src_orig[start : start + length]),
            "text": " ".join(t[1] for t in toks[i:j]),
        }
        i = j


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="*", default=["data", "docs"], help="repo paths to scan")
    ap.add_argument("--sources", default=os.environ.get("CNA_SOURCES"), help="cna-sources folder")
    ap.add_argument("--min-words", type=int, default=10)
    ap.add_argument("--include-names", action="store_true", help="also report name runs")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    if not args.sources:
        raise SystemExit("set CNA_SOURCES or pass --sources")
    n = args.min_words
    src_lower, src_orig = load_sources(Path(args.sources))
    grams: dict = {}
    for k in range(len(src_lower) - n + 1):
        grams.setdefault(tuple(src_lower[k : k + n]), k)

    files: list[Path] = []
    for p in args.paths:
        root = (REPO / p).resolve()
        if root.is_file():
            files.append(root)
        else:
            files.extend(f for f in sorted(root.rglob("*")) if f.suffix in TEXT_SUFFIXES and f.is_file())
    hits = [h for f in files for h in scan_file(f, n, grams, src_orig)]
    shown = [h for h in hits if args.include_names or not h["name"]]
    if args.json:
        print(json.dumps(shown, indent=1))
    else:
        for h in sorted(shown, key=lambda h: (h["file"], h["line"])):
            tag = " [name]" if h["name"] else ""
            print(f"{h['file']}:{h['line']}: {h['words']} words{tag}: {h['text']}")
        names = sum(1 for h in hits if h["name"])
        prose = sum(1 for h in hits if not h["name"])
        print(
            f"\n{len(files)} files scanned; {prose} verbatim prose run(s) of {n}+ words"
            f"; {names} name run(s) {'shown' if args.include_names else 'ignored'}",
            file=sys.stderr,
        )
    return 1 if any(not h["name"] for h in hits) else 0


if __name__ == "__main__":
    sys.exit(main())
