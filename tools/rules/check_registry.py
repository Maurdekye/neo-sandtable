#!/usr/bin/env python3
"""Completeness and schema checker for the rule-case registry (data/rules/<book>/).

Compares every case number found in a source rulebook with the cases registered in
data/rules/<book>/NN-*.toml, and (with --validate) checks each record against the schema in
data/rules/README.md.

Usage:
    CNA_SOURCES=/path/to/cna-sources python tools/rules/check_registry.py --book land
    python tools/rules/check_registry.py --book all --validate
    python tools/rules/check_registry.py --book land --sources /path/to/cna-sources --json

Sources are read from $CNA_SOURCES (or --sources) and are never copied anywhere. If the sources
are not available the tool still validates the registry (--validate) and reports that the
completeness comparison was skipped.

Source case extraction: every bracketed case id such as [8.37], [4.44b] or [10.0] found anywhere
in the book's text (rules-2021/<file>.txt). A section header [N.0] is satisfied by the file's
[section] record unless the registry also lists it as a [[case]] (use that for sections that carry
rules text of their own); registering the same id twice as a [[case]] is a duplicate.

Exit status: 0 = complete and valid, 1 = missing/extra/duplicate/invalid, 2 = usage or I/O error.
Requires Python >= 3.11 for --validate (tomllib); coverage-only runs work on any Python 3.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

BOOKS = {
    "land": {"text": "land-2021.txt", "dir": "land"},
    "airlog": {"text": "air-logistics-2021.txt", "dir": "airlog"},
    "scen": {"text": "scenarios-2021.txt", "dir": "scen"},
}

CASE_RE = re.compile(r"\[(\d+\.\d+[a-z]?)\]")
ID_RE = re.compile(r"^\d+\.\d+[a-z]?$")

KINDS = {"rule", "procedure", "definition", "table", "example", "commentary",
         "designer_note", "addendum"}
DISPOSITIONS = {"automatic", "decision", "data", "display", "superseded", "none", "unresolved"}
SEATS = {"commander", "front_line", "rear_area", "logistics", "air", "naval", "either", "none"}
SYSTEMS = {"land", "air", "logistics", "naval"}
GRAZIANI = {"yes", "no", "conditional"}
TRIGGERS = {"scheduled", "triggered", "standing"}
SECRECY = {"open", "secret", "secret_simultaneous"}


def read_text(path: Path) -> str:
    raw = path.read_bytes()
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("cp1252", errors="replace")


def source_cases(text: str) -> tuple[list[str], list[str], dict[str, int]]:
    """Return (ordered unique ids, ids seen only mid-line, id -> occurrence count)."""
    ids: list[str] = []
    counts: Counter[str] = Counter()
    line_start: set[str] = set()
    for line in text.split("\n"):
        stripped = line.lstrip("\x0c ")
        m0 = re.match(r"^(?:\(addition:?\)\s*)?\[(\d+\.\d+[a-z]?)\]", stripped)
        if m0:
            line_start.add(m0.group(1))
        for m in CASE_RE.finditer(line):
            cid = m.group(1)
            counts[cid] += 1
            if cid not in ids:
                ids.append(cid)
    midline_only = [i for i in ids if i not in line_start]
    return ids, midline_only, dict(counts)


def parse_timing_vocab(readme: Path) -> tuple[set[str], list[str]]:
    """Read the timing vocabulary block from data/rules/README.md."""
    text = readme.read_text(encoding="utf-8")
    m = re.search(r"### `timing` vocabulary.*?```\n(.*?)```", text, re.S)
    if not m:
        raise SystemExit("cannot find the timing vocabulary block in data/rules/README.md")
    exact: set[str] = set()
    wildcards: list[str] = []
    for raw in m.group(1).splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        parts = [p.strip() for p in line.split("|")]
        base = parts[0]
        if base.endswith(".<phase>"):
            wildcards.append(base[: -len("<phase>")])
            continue
        exact.add(base)
        parent = base.rsplit(".", 1)[0] if "." in base else ""
        for p in parts[1:]:
            if p.startswith("."):
                exact.add(f"{parent}{p}")
    return exact, wildcards


def timing_ok(anchor: str, exact: set[str], wildcards: list[str]) -> bool:
    if anchor in exact:
        return True
    if any(anchor.startswith(w) and len(anchor) > len(w) for w in wildcards):
        return True
    # a parent group of a known anchor is allowed for rules that span the whole group
    return any(e.startswith(anchor + ".") for e in exact)


def load_toml(path: Path):
    try:
        import tomllib  # type: ignore
    except ModuleNotFoundError:
        try:
            import tomli as tomllib  # type: ignore
        except ModuleNotFoundError:
            raise SystemExit("--validate needs Python >= 3.11 (tomllib) or the tomli package")
    with path.open("rb") as f:
        return tomllib.load(f)


def table_ids(book: str) -> set[str]:
    out: set[str] = set()
    d = REPO / "data" / "tables" / BOOKS[book]["dir"]
    if not d.is_dir():
        return out
    for p in sorted(d.glob("*.toml")):
        m = re.search(r'^\s*id\s*=\s*"([^"]+)"', p.read_text(encoding="utf-8"), re.M)
        if m:
            out.add(m.group(1))
    return out


def registry_files(book: str) -> list[Path]:
    d = REPO / "data" / "rules" / BOOKS[book]["dir"]
    return sorted(d.glob("[0-9][0-9]-*.toml")) if d.is_dir() else []


def load_registry(book: str, validate: bool):
    """Return (case ids in book order with duplicates, section ids, errors, case records)."""
    cases: list[str] = []
    sections: list[str] = []
    errors: list[str] = []
    records: list[tuple[str, dict]] = []
    for p in registry_files(book):
        rel = p.relative_to(REPO).as_posix()
        if validate:
            try:
                doc = load_toml(p)
            except Exception as e:  # tomllib.TOMLDecodeError and friends
                errors.append(f"{rel}: TOML parse error: {e}")
                continue
            sec = doc.get("section")
            if sec:
                sections.append(str(sec.get("id", "")))
            for c in doc.get("case", []):
                cases.append(str(c.get("id", "")))
                records.append((rel, c))
            errors.extend(validate_section(rel, book, sec))
        else:
            text = p.read_text(encoding="utf-8")
            m = re.search(r"\[section\][^\[]*?\bid\s*=\s*\"([^\"]+)\"", text, re.S)
            if m:
                sections.append(m.group(1))
            for m in re.finditer(r"^\[\[case\]\]\s*\n(?:[^\[]*?\n)?\s*id\s*=\s*\"([^\"]+)\"",
                                 text, re.M):
                cases.append(m.group(1))
    return cases, sections, errors, records


def validate_section(rel: str, book: str, sec) -> list[str]:
    errs: list[str] = []
    if not sec:
        return [f"{rel}: missing [section]"]
    if sec.get("book") != book:
        errs.append(f"{rel}: [section].book must be {book!r}")
    if not re.match(r"^\d+\.0$", str(sec.get("id", ""))):
        errs.append(f"{rel}: [section].id must look like '8.0'")
    if not sec.get("title"):
        errs.append(f"{rel}: [section].title missing")
    systems = sec.get("systems")
    if not isinstance(systems, list) or not systems or not set(systems) <= SYSTEMS:
        errs.append(f"{rel}: [section].systems must be a non-empty list drawn from {sorted(SYSTEMS)}")
    return errs


def validate_case(rel: str, c: dict, exact: set[str], wild: list[str], tables: set[str]) -> list[str]:
    cid = c.get("id", "?")
    pre = f"{rel} [{cid}]"
    errs: list[str] = []
    if not ID_RE.match(str(cid)):
        errs.append(f"{pre}: bad id")
    for f in ("title", "summary", "kind", "disposition", "seat", "timing", "src"):
        if f not in c:
            errs.append(f"{pre}: missing field {f}")
    if c.get("kind") not in KINDS:
        errs.append(f"{pre}: kind {c.get('kind')!r} not in {sorted(KINDS)}")
    disp = c.get("disposition")
    if disp not in DISPOSITIONS:
        errs.append(f"{pre}: disposition {disp!r} not in {sorted(DISPOSITIONS)}")
    seat = c.get("seat")
    if seat not in SEATS:
        errs.append(f"{pre}: seat {seat!r} not in {sorted(SEATS)}")
    if disp == "decision" and seat == "none":
        errs.append(f"{pre}: decision needs a seat")
    if disp != "decision" and seat != "none":
        errs.append(f"{pre}: seat must be 'none' unless disposition = decision")
    summary = str(c.get("summary", ""))
    if len(summary.strip()) < 10:
        errs.append(f"{pre}: summary is empty or too short")
    if len(summary) > 700:
        errs.append(f"{pre}: summary is {len(summary)} chars; keep it to 1-3 sentences (<=700)")
    timing = c.get("timing")
    if not isinstance(timing, list):
        errs.append(f"{pre}: timing must be a list")
    else:
        for t in timing:
            if not timing_ok(t, exact, wild):
                errs.append(f"{pre}: timing anchor {t!r} is not in the vocabulary")
        if disp == "decision" and not timing:
            errs.append(f"{pre}: decision needs at least one timing anchor")
    src = c.get("src")
    if not isinstance(src, list) or not src or not all(isinstance(s, str) and ":" in s for s in src):
        errs.append(f"{pre}: src must be a non-empty list of 'book:case' citations")
    for t in c.get("tables", []):
        if t not in tables:
            errs.append(f"{pre}: table {t!r} not found in data/tables")
    applies = c.get("applies")
    if not isinstance(applies, dict) or applies.get("graziani") not in GRAZIANI:
        errs.append(f"{pre}: [case.applies].graziani must be one of {sorted(GRAZIANI)}")
    else:
        if applies["graziani"] in {"no", "conditional"} and not str(applies.get("graziani_note", "")).strip():
            errs.append(f"{pre}: graziani_note required when graziani is {applies['graziani']!r}")
    if disp == "decision":
        d = c.get("decision")
        if not isinstance(d, dict):
            errs.append(f"{pre}: decision case needs a [case.decision] table")
        else:
            for f in ("what", "params", "trigger", "secrecy", "interrupts"):
                if f not in d:
                    errs.append(f"{pre}: [case.decision] missing {f}")
            if d.get("trigger") not in TRIGGERS:
                errs.append(f"{pre}: decision.trigger not in {sorted(TRIGGERS)}")
            if d.get("secrecy") not in SECRECY:
                errs.append(f"{pre}: decision.secrecy not in {sorted(SECRECY)}")
            if not isinstance(d.get("interrupts"), bool):
                errs.append(f"{pre}: decision.interrupts must be a bool")
    elif "decision" in c:
        errs.append(f"{pre}: [case.decision] present but disposition is {disp!r}")
    if disp == "superseded" and not c.get("errata"):
        errs.append(f"{pre}: superseded case must cite errata")
    if disp == "unresolved" and not (c.get("interp") or c.get("gaps")):
        errs.append(f"{pre}: unresolved case needs interp = [...] or gaps = [...] entries")
    return errs


def check_book(book: str, sources: Path | None, validate: bool) -> dict:
    result: dict = {"book": book}
    cases, sections, errors, records = load_registry(book, validate)
    reg_counts = Counter(cases)
    result["registry_cases"] = len(set(cases))
    result["registry_sections"] = len(set(sections))
    result["duplicates"] = sorted(k for k, v in reg_counts.items() if v > 1)
    if validate:
        exact, wild = parse_timing_vocab(REPO / "data" / "rules" / "README.md")
        tables = table_ids(book)
        for rel, c in records:
            errors.extend(validate_case(rel, c, exact, wild, tables))
    result["validation_errors"] = errors if validate else None

    covered = set(cases) | set(sections)
    if sources is None:
        result["source_cases"] = None
        result["skipped"] = "CNA_SOURCES not set; completeness comparison skipped"
        return result
    path = sources / "rules-2021" / BOOKS[book]["text"]
    if not path.is_file():
        result["source_cases"] = None
        result["skipped"] = f"source text not found: {path}"
        return result
    ids, midline, counts = source_cases(read_text(path))
    result["source_cases"] = len(ids)
    result["source_midline_only"] = midline
    result["source_repeated_ids"] = sorted(k for k, v in counts.items() if v > 1)
    result["missing"] = [i for i in ids if i not in covered]
    result["extra"] = sorted(i for i in covered if i not in set(ids))
    result["coverage_pct"] = round(100.0 * (len(ids) - len(result["missing"])) / max(len(ids), 1), 2)
    return result


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--book", choices=[*BOOKS, "all"], default="land")
    ap.add_argument("--sources", help="path to cna-sources (default: $CNA_SOURCES)")
    ap.add_argument("--validate", action="store_true", help="also validate every record's schema")
    ap.add_argument("--json", action="store_true", help="print machine-readable JSON")
    ap.add_argument("--allow-incomplete", action="store_true",
                    help="report missing cases but do not fail (work in progress)")
    ap.add_argument("--section", action="append", type=int,
                    help="restrict the missing/extra lists to these major sections (repeatable)")
    args = ap.parse_args()

    src_env = args.sources or os.environ.get("CNA_SOURCES")
    sources = Path(src_env) if src_env else None
    books = list(BOOKS) if args.book == "all" else [args.book]
    results = [check_book(b, sources, args.validate) for b in books]

    if args.section:
        keep = {str(s) for s in args.section}
        for r in results:
            for k in ("missing", "extra"):
                if k in r:
                    r[k] = [i for i in r[k] if i.split(".")[0] in keep]

    if args.json:
        print(json.dumps(results, indent=2))
    else:
        for r in results:
            print(f"== {r['book']}")
            print(f"  cases in registry: {r['registry_cases']} (+{r['registry_sections']} section records)")
            if r.get("source_cases") is None:
                print(f"  {r['skipped']}")
            else:
                print(f"  cases in source:   {r['source_cases']}")
                print(f"  coverage:          {r['coverage_pct']}%")
                print(f"  missing ({len(r['missing'])}): {', '.join(r['missing'][:60])}"
                      f"{' ...' if len(r['missing']) > 60 else ''}")
                print(f"  extra   ({len(r['extra'])}): {', '.join(r['extra'])}")
                if r["source_midline_only"]:
                    print("  note: ids seen only mid-line in the source (verify they are cases, "
                          f"not cross-references): {', '.join(r['source_midline_only'])}")
                if r["source_repeated_ids"]:
                    print("  note: ids printed more than once in the source: "
                          f"{', '.join(r['source_repeated_ids'])}")
            print(f"  duplicates in registry ({len(r['duplicates'])}): {', '.join(r['duplicates'])}")
            if r["validation_errors"] is not None:
                print(f"  validation errors: {len(r['validation_errors'])}")
                for e in r["validation_errors"][:80]:
                    print(f"    - {e}")

    bad = False
    for r in results:
        if r["duplicates"] or r.get("validation_errors"):
            bad = True
        if r.get("source_cases") is not None:
            if r["extra"] or (r["missing"] and not args.allow_incomplete):
                bad = True
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
