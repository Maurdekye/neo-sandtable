#!/usr/bin/env python3
"""Rule coverage report: the rule-case registry versus case citations in the engine source.

Reads data/rules/**/*.toml (the registry) and crates/cna-rules/{src,tests}/**/*.rs (the engine),
and reports, per scenario and per sequence-of-play anchor, how many applicable procedural cases
the engine implements, tests, explicitly marks unsupported, or is still missing. No CNA_SOURCES
is needed.

Usage:
    python tools/rules/coverage.py [--scenario graziani] [--json out.json] [--markdown out.md]
                                   [--registry DIR] [--src DIR ...] [--require-complete]
    python tools/rules/coverage.py --self-test

Exit status: 0 = report produced and no citation errors; 1 = citation errors (unknown or
malformed citations, unsupported markers without a reason) or, with --require-complete, missing
cases; 2 = usage / I/O error. Missing cases alone never fail a default run (the report is
informational); unknown citations always do.

## Convention read from the engine source

    /// Cases: land:8.31, land:8.32, airlog:49.13     (also `//!` and plain `//`)
    /// Unsupported: land:8.44 [@graziani] - why the game stops when this arises

* Citation = `<book>:<id>` with book in land | airlog | scen and id as printed (`8.37`, `4.44b`).
  A line may continue onto the next comment line if it ends with a comma.
* `orig79:<book>:<case>` on Cases lines cites the original 1979 printing. Its format is
  checked and its locations are reported in `original_citations`, outside all 2021 counts.
* A case is *implemented* when a `Cases:` line outside test code cites it; *tested* when it is
  implemented and also cited by a `Cases:` line in test code (inside a `#[cfg(test)]` item, a
  file with an inner `#![cfg(test)]`, a `tests.rs`, or any file under a `tests/` directory).
  A citation found only in test code is reported as a warning and counts as neither.
* `Unsupported:` marks a case explicitly unsupported (reaching it stops the game). An optional
  `@name[,name]` token limits it to those scenarios; the reason after ` - ` (or an em dash) is
  required. A case that is both implemented and marked unsupported counts as implemented
  (warning).
* A citation of a section id such as `land:8.0` is accepted when the registry has that section.

## Applicability

A case is applicable to a scenario when its disposition is `automatic` or `decision` and
`[case.applies].<scenario>` is `yes` or `conditional`. Scenarios are the keys found in the
registry's `[case.applies]` tables. missing = applicable - implemented - unsupported.
Anchors follow `data/rules/README.md` (vocabulary order); a case listing several anchors is
counted in each, while the scenario totals count every case once. Cases with no anchor go under
`(none)`; anchors absent from the vocabulary are listed after the known ones with a warning.

## JSON output (schema 1)

    {
      "schema": 1,
      "inputs": {"registry_cases": int, "source_files": int, "crate_present": bool},
      "scenarios": {
        "<scenario>": {
          "applicable": int, "implemented": int, "tested": int, "unsupported": int,
          "missing": int, "percent_implemented": float, "complete": bool,
          "missing_ids": ["land:8.31", ...],           # registry order
          "by_anchor": [                                # vocabulary order
            {"anchor": str, "applicable": int, "implemented": int, "tested": int,
             "unsupported": int, "missing": int, "missing_ids": [str, ...]}, ...]
        }, ...},
      "citations": {"land:8.31": {"implemented": bool, "tested": bool,
                                  "unsupported": [scenario|"*", ...],
                                  "locations": ["crates/cna-rules/src/x.rs:12", ...]}, ...},
      "errors":   [{"kind": "unknown_citation"|"malformed_citation"|"unsupported_without_reason",
                    "location": str, "text": str}, ...],
      "warnings": [{"kind": str, "location": str, "text": str}, ...]
    }
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
BOOKS = ("land", "airlog", "scen")
PROCEDURAL = {"automatic", "decision"}
NONE_ANCHOR = "(none)"

ORIGINAL_RE = re.compile(r"^orig79:(land|airlog|scen):(\d+\.\d+[a-z]?)$")
CITE_RE = re.compile(r"^(land|airlog|scen):(\d+\.\d+[a-z]?)$")
CASES_RE = re.compile(r"^\s*(?://[/!]?)\s*Cases:\s*(.*?)\s*$")
UNSUPPORTED_RE = re.compile(r"^\s*(?://[/!]?)\s*Unsupported:\s*(.*?)\s*$")
CONT_RE = re.compile(r"^\s*(?://[/!]?)\s*(.*?)\s*$")
REASON_SPLIT = re.compile(r"\s+(?:—|–|--|-)\s+")
CFG_TEST_RE = re.compile(r"#\[cfg\(\s*test\s*\)\]")
CFG_TEST_INNER_RE = re.compile(r"#!\[cfg\(\s*test\s*\)\]")


def load_toml(path: Path) -> dict:
    try:
        import tomllib  # type: ignore
    except ModuleNotFoundError:
        try:
            import tomli as tomllib  # type: ignore
        except ModuleNotFoundError:
            raise SystemExit("coverage.py needs Python >= 3.11 (tomllib) or the tomli package")
    with open(path, "rb") as f:
        return tomllib.load(f)


# --------------------------------------------------------------------------- registry

def anchor_vocabulary(readme: Path) -> list[str]:
    """Ordered anchors from the timing-vocabulary code block of data/rules/README.md."""
    text = readme.read_text(encoding="utf-8")
    m = re.search(r"### `timing` vocabulary.*?```\n(.*?)```", text, re.S)
    if not m:
        return []
    out: list[str] = []
    for line in m.group(1).split("\n"):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        prefix = ""
        for i, tok in enumerate(t.strip() for t in line.split("|")):
            if not tok:
                continue
            if i == 0:
                prefix = tok.rsplit(".", 1)[0] + "." if "." in tok else ""
                out.append(tok)
            elif tok.startswith("."):
                out.append(prefix + tok[1:])
            else:
                out.append(tok)
    return out


def load_registry(reg_dir: Path) -> tuple[dict[str, dict], set[str]]:
    """Return (case key -> record in registry order, set of known section keys)."""
    cases: dict[str, dict] = {}
    sections: set[str] = set()
    for path in sorted(reg_dir.glob("*/*.toml")):
        data = load_toml(path)
        sec = data.get("section", {})
        book = sec.get("book", path.parent.name)
        if sec.get("id"):
            sections.add(f"{book}:{sec['id']}")
        for c in data.get("case", []):
            cases[f"{book}:{c['id']}"] = c
    return cases, sections


def scenarios_in(cases: dict[str, dict]) -> list[str]:
    seen: list[str] = []
    for c in cases.values():
        for k in c.get("applies", {}):
            if not k.endswith("_note") and k not in seen:
                seen.append(k)
    return seen


# --------------------------------------------------------------------------- source scan

def _strip_code(line: str) -> str:
    """Drop string/char literals and line comments so braces can be counted."""
    line = re.sub(r'"(?:\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(?:\\.|[^'\\])'", "''", line)
    return line.split("//", 1)[0]


def scan_file(path: Path, rel: str, file_is_test: bool):
    """Yield (kind, tokens, in_test, location, raw) for every Cases/Unsupported line.

    kind: "cases" | "unsupported"; tokens: the text after the marker (continuations joined).
    """
    lines = path.read_text(encoding="utf-8", errors="replace").split("\n")
    depth = 0
    test_depth: list[int] = []     # brace depths at which test regions were opened
    pending = False                # saw #[cfg(test)], waiting for the item's opening brace
    out = []
    i = 0
    while i < len(lines):
        raw = lines[i]
        in_test = file_is_test or bool(test_depth)
        if CFG_TEST_INNER_RE.search(raw):
            file_is_test = in_test = True
        m = CASES_RE.match(raw)
        u = UNSUPPORTED_RE.match(raw)
        if m or u:
            kind = "cases" if m else "unsupported"
            body = (m or u).group(1)
            loc = f"{rel}:{i + 1}"
            while kind == "cases" and body.endswith(",") and i + 1 < len(lines):
                c = CONT_RE.match(lines[i + 1])
                if not c or not lines[i + 1].lstrip().startswith("//") \
                        or CASES_RE.match(lines[i + 1]) or UNSUPPORTED_RE.match(lines[i + 1]):
                    break
                i += 1
                body += " " + c.group(1)
            out.append((kind, body, in_test, loc, raw.strip()))
            i += 1
            continue
        code = _strip_code(raw)
        if CFG_TEST_RE.search(code):
            pending = True
        elif pending and code.strip() and not code.strip().startswith("#["):
            if "{" not in code and ";" in code:
                pending = False                       # `#[cfg(test)] mod tests;` etc.
            elif "{" in code:
                test_depth.append(depth)
                pending = False
        depth += code.count("{") - code.count("}")
        while test_depth and depth <= test_depth[-1]:
            test_depth.pop()
        i += 1
    return out


def source_files(dirs: list[Path]) -> list[tuple[Path, str, bool]]:
    files = []
    for d in dirs:
        if not d.is_dir():
            continue
        for p in sorted(d.rglob("*.rs")):
            parts = p.relative_to(d).parts
            is_test = "tests" in parts[:-1] or p.name in ("tests.rs", "test.rs") \
                or d.name == "tests"
            try:
                rel = p.relative_to(REPO).as_posix()
            except ValueError:
                rel = p.as_posix()
            files.append((p, rel, is_test))
    return files


def parse_citations(files, cases, sections):
    cites: dict[str, dict] = {}
    errors: list[dict] = []
    warnings: list[dict] = []
    original: dict[str, dict] = {}

    def entry(key):
        return cites.setdefault(key, {"impl_loc": [], "test_loc": [], "unsupported": []})

    for path, rel, is_test in files:
        for kind, body, in_test, loc, raw in scan_file(path, rel, is_test):
            if kind == "cases":
                for tok in (t.strip().rstrip(".;") for t in body.split(",")):
                    if not tok:
                        continue
                    if ORIGINAL_RE.fullmatch(tok):
                        e = original.setdefault(tok, {"impl_loc": [], "test_loc": []})
                        e["test_loc" if in_test else "impl_loc"].append(loc)
                        continue
                    m = CITE_RE.match(tok)
                    if not m:
                        errors.append({"kind": "malformed_citation", "location": loc, "text": tok})
                        continue
                    key = tok
                    if key not in cases and key not in sections:
                        errors.append({"kind": "unknown_citation", "location": loc, "text": key})
                        continue
                    entry(key)["test_loc" if in_test else "impl_loc"].append(loc)
            else:
                parts = REASON_SPLIT.split(body, maxsplit=1)
                reason = parts[1].strip() if len(parts) == 2 else ""
                if not reason:
                    errors.append({"kind": "unsupported_without_reason", "location": loc,
                                   "text": raw})
                profiles: list[str] = []
                toks = []
                for t in re.split(r"[,\s]+", parts[0]):
                    if t.startswith("@"):
                        profiles += [p for p in t[1:].split(",") if p]
                    elif t:
                        toks.append(t)
                for tok in toks:
                    if not CITE_RE.match(tok):
                        errors.append({"kind": "malformed_citation", "location": loc, "text": tok})
                    elif tok not in cases and tok not in sections:
                        errors.append({"kind": "unknown_citation", "location": loc, "text": tok})
                    else:
                        entry(tok)["unsupported"] += profiles or ["*"]
    for key, e in sorted(cites.items()):
        if e["test_loc"] and not e["impl_loc"] and not e["unsupported"]:
            warnings.append({"kind": "test_only_citation", "location": e["test_loc"][0],
                             "text": f"{key} is cited only in test code"})
    return cites, original, errors, warnings


# --------------------------------------------------------------------------- analysis

def analyze(registry_dir: Path, src_dirs: list[Path], only: str | None = None) -> dict:
    cases, sections = load_registry(registry_dir)
    vocab = anchor_vocabulary(registry_dir / "README.md")
    files = source_files(src_dirs)
    cites, original, errors, warnings = parse_citations(files, cases, sections)
    scenarios = [only] if only else scenarios_in(cases)
    if only and only not in scenarios_in(cases):
        raise SystemExit(f"unknown scenario {only!r}; registry has {scenarios_in(cases)}")

    unknown_anchors: list[str] = []
    result_scn: dict[str, dict] = {}
    for scn in scenarios:
        rows: dict[str, dict] = {}
        total = {"applicable": 0, "implemented": 0, "tested": 0, "unsupported": 0, "missing": 0}
        missing_all: list[str] = []
        for key, c in cases.items():
            if c.get("disposition") not in PROCEDURAL:
                continue
            if c.get("applies", {}).get(scn) not in ("yes", "conditional"):
                continue
            e = cites.get(key)
            impl = bool(e and e["impl_loc"])
            tested = impl and bool(e["test_loc"])
            unsup = (not impl) and bool(e and (scn in e["unsupported"] or "*" in e["unsupported"]))
            if impl and e["unsupported"] and (scn in e["unsupported"] or "*" in e["unsupported"]):
                warnings.append({"kind": "implemented_and_unsupported",
                                 "location": e["impl_loc"][0],
                                 "text": f"{key} is cited as implemented and as unsupported"})
            miss = not impl and not unsup
            anchors = c.get("timing") or [NONE_ANCHOR]
            for a in anchors:
                if a not in vocab and a != NONE_ANCHOR and a not in unknown_anchors:
                    unknown_anchors.append(a)
                row = rows.setdefault(a, {"anchor": a, "applicable": 0, "implemented": 0,
                                          "tested": 0, "unsupported": 0, "missing": 0,
                                          "missing_ids": []})
                row["applicable"] += 1
                row["implemented"] += impl
                row["tested"] += tested
                row["unsupported"] += unsup
                row["missing"] += miss
                if miss:
                    row["missing_ids"].append(key)
            total["applicable"] += 1
            total["implemented"] += impl
            total["tested"] += tested
            total["unsupported"] += unsup
            total["missing"] += miss
            if miss:
                missing_all.append(key)
        order = {a: i for i, a in enumerate(vocab)}
        by_anchor = sorted(rows.values(),
                           key=lambda r: (order.get(r["anchor"], len(vocab) + 1
                                                    + (r["anchor"] == NONE_ANCHOR)), r["anchor"]))
        pct = round(100.0 * total["implemented"] / total["applicable"], 1) \
            if total["applicable"] else 100.0
        result_scn[scn] = {**total, "percent_implemented": pct,
                           "complete": total["missing"] == 0, "missing_ids": missing_all,
                           "by_anchor": by_anchor}
    for a in unknown_anchors:
        warnings.append({"kind": "anchor_not_in_vocabulary", "location": "data/rules",
                         "text": a})
    return {
        "schema": 1,
        "inputs": {"registry_cases": len(cases), "source_files": len(files),
                   "crate_present": any(d.is_dir() for d in src_dirs)},
        "scenarios": result_scn,
        "citations": {k: {"implemented": bool(e["impl_loc"]),
                          "tested": bool(e["impl_loc"] and e["test_loc"]),
                          "unsupported": sorted(set(e["unsupported"])),
                          "locations": e["impl_loc"] + e["test_loc"]}
                      for k, e in sorted(cites.items())},
        "original_citations": [{"citation": k, "implemented": bool(e["impl_loc"]),
                                "tested": bool(e["impl_loc"] and e["test_loc"]),
                                "locations": e["impl_loc"] + e["test_loc"]}
                               for k, e in sorted(original.items())],
        "errors": errors,
        "warnings": warnings,
    }


# --------------------------------------------------------------------------- output

def markdown(rep: dict) -> str:
    out = ["# Rule coverage report", ""]
    inp = rep["inputs"]
    out.append(f"Registry cases: {inp['registry_cases']}; engine source files scanned: "
               f"{inp['source_files']}" + ("" if inp["crate_present"]
                                           else " (engine crate not found)") + ".")
    out.append("")
    out += ["| Scenario | Applicable | Implemented | Tested | Unsupported | Missing | Implemented % |",
            "|---|---:|---:|---:|---:|---:|---:|"]
    for name, s in rep["scenarios"].items():
        out.append(f"| {name} | {s['applicable']} | {s['implemented']} | {s['tested']} | "
                   f"{s['unsupported']} | {s['missing']} | {s['percent_implemented']} |")
    for name, s in rep["scenarios"].items():
        out += ["", f"## {name}: by sequence-of-play anchor", "",
                "| Anchor | Applicable | Implemented | Tested | Unsupported | Missing |",
                "|---|---:|---:|---:|---:|---:|"]
        for r in s["by_anchor"]:
            out.append(f"| `{r['anchor']}` | {r['applicable']} | {r['implemented']} | "
                       f"{r['tested']} | {r['unsupported']} | {r['missing']} |")
        out.append("")
        for r in s["by_anchor"]:
            if r["missing_ids"]:
                out.append(f"<details><summary><code>{r['anchor']}</code>: "
                           f"{r['missing']} missing</summary>\n")
                out.append(", ".join(r["missing_ids"]))
                out.append("\n</details>\n")
    if rep.get("original_citations"):
        out += ["", "## Original 1979 citations", "",
                "These citations are outside the 2021 registry coverage counts.", ""]
        out += [f"- `{e['citation']}`: " + ", ".join(f"`{p}`" for p in e['locations'])
                for e in rep["original_citations"]]
        out.append("")
    if rep["errors"]:
        out += ["## Citation errors", ""]
        out += [f"- `{e['location']}` {e['kind']}: `{e['text']}`" for e in rep["errors"]]
        out.append("")
    if rep["warnings"]:
        out += ["## Warnings", ""]
        out += [f"- `{w['location']}` {w['kind']}: {w['text']}" for w in rep["warnings"]]
        out.append("")
    return "\n".join(out)


def summary_text(rep: dict) -> str:
    lines = []
    for name, s in rep["scenarios"].items():
        lines.append(f"{name}: {s['applicable']} applicable, {s['implemented']} implemented "
                     f"({s['percent_implemented']}%), {s['tested']} tested, "
                     f"{s['unsupported']} unsupported, {s['missing']} missing")
    for e in rep["errors"]:
        lines.append(f"ERROR {e['location']} {e['kind']}: {e['text']}")
    return "\n".join(lines)


# --------------------------------------------------------------------------- self-test

FIXTURE_README = """# Registry

### `timing` vocabulary

```
setup
initiative
opstage.weather
opstage.movement_and_combat.movement
opstage.movement_and_combat.combat.position | .barrage
end_of_game
```
"""

FIXTURE_LAND = """
[section]
id = "1.0"
book = "land"
title = "T"

[[case]]
id = "1.1"
disposition = "automatic"
timing = ["setup"]
[case.applies]
graziani = "yes"
other = "no"

[[case]]
id = "1.2"
disposition = "decision"
timing = ["opstage.movement_and_combat.movement", "opstage.weather"]
[case.applies]
graziani = "conditional"
other = "yes"

[[case]]
id = "1.3"
disposition = "automatic"
timing = ["opstage.movement_and_combat.combat.barrage"]
[case.applies]
graziani = "yes"

[[case]]
id = "1.4"
disposition = "automatic"
timing = []
[case.applies]
graziani = "yes"

[[case]]
id = "1.5"
disposition = "data"
timing = ["setup"]
[case.applies]
graziani = "yes"

[[case]]
id = "1.6"
disposition = "automatic"
timing = ["made_up.anchor"]
[case.applies]
graziani = "yes"
"""

FIXTURE_RS = """
//! Cases: land:1.0
/// Cases: land:1.1,
/// land:1.2
pub fn a() { let s = "{ not a brace"; }

// Cases: land:1.5
fn b() {}

/// Unsupported: land:1.3 @graziani - barrage is deferred
/// Unsupported: land:1.4

#[cfg(test)]
mod tests {
    /// Cases: land:1.1, land:1.6
    #[test]
    fn t() { if true { } }
}

/// Cases: land:1.9, land:bad
fn c() {}
"""


def self_test() -> int:
    ok = True

    def check(cond, msg):
        nonlocal ok
        if not cond:
            ok = False
            print(f"SELF-TEST FAIL: {msg}")

    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        reg = root / "rules"
        (reg / "land").mkdir(parents=True)
        (reg / "README.md").write_text(FIXTURE_README, encoding="utf-8")
        (reg / "land" / "01-x.toml").write_text(FIXTURE_LAND, encoding="utf-8")
        src = root / "crate" / "src"
        src.mkdir(parents=True)
        (src / "lib.rs").write_text(FIXTURE_RS, encoding="utf-8")
        tdir = root / "crate" / "tests"
        tdir.mkdir()
        (tdir / "it.rs").write_text("// Cases: land:1.2\nfn x() {}\n", encoding="utf-8")

        rep = analyze(reg, [root / "crate" / "src", root / "crate" / "tests"])
        g = rep["scenarios"]["graziani"]
        check(g["applicable"] == 5, f"graziani applicable {g['applicable']} != 5")
        check(g["implemented"] == 2, f"graziani implemented {g['implemented']} != 2")
        check(g["tested"] == 2, f"graziani tested {g['tested']} != 2")
        check(g["unsupported"] == 2, f"graziani unsupported {g['unsupported']} != 2")
        check(g["missing_ids"] == ["land:1.6"],
              f"graziani missing {g['missing_ids']}")
        check(g["missing"] == 1, "graziani missing count")
        check(not g["complete"], "graziani must not be complete")
        o = rep["scenarios"]["other"]
        check(o["applicable"] == 1 and o["implemented"] == 1 and o["complete"],
              f"other scenario {o}")
        names = [r["anchor"] for r in g["by_anchor"]]
        check(names == ["setup", "opstage.weather", "opstage.movement_and_combat.movement",
                        "opstage.movement_and_combat.combat.barrage", "made_up.anchor",
                        "(none)"], f"anchor order {names}")
        kinds = sorted(e["kind"] for e in rep["errors"])
        check(kinds == ["malformed_citation", "unknown_citation", "unsupported_without_reason"],
              f"errors {kinds}")
        check(any(w["kind"] == "test_only_citation" and "land:1.6" in w["text"]
                  for w in rep["warnings"]), "test-only warning for land:1.6")
        check(any(w["kind"] == "anchor_not_in_vocabulary" for w in rep["warnings"]),
              "unknown anchor warning")
        check(rep["citations"]["land:1.1"]["tested"], "land:1.1 tested via cfg(test) module")
        check(rep["citations"]["land:1.5"]["implemented"], "land:1.5 cited")
        check("land:1.9" not in rep["citations"], "unknown citation not recorded")
        check(rep["inputs"]["registry_cases"] == 6, "registry case count")
        check("Rule coverage report" in markdown(rep) and json.dumps(rep), "renders")

        # Code after the test module must not be classed as test code.
        loc_impl = rep["citations"]["land:1.1"]["locations"]
        check(len(loc_impl) == 2, f"land:1.1 locations {loc_impl}")

        # Clean tree: no errors, scenario filter works, missing crate is tolerated.
        (src / "lib.rs").write_text("/// Cases: land:1.1, land:1.2\nfn a() {}\n",
                                    encoding="utf-8")
        rep2 = analyze(reg, [src], only="graziani")
        check(not rep2["errors"] and list(rep2["scenarios"]) == ["graziani"], "clean run")
        (src / "lib.rs").write_text("/// Cases: orig79:land:4.22, orig79:airlog:49.12\nfn a() {}\n"
                                    "#[cfg(test)]\nmod tests {\n/// Cases: orig79:land:4.22\nfn b() {}\n}\n",
                                    encoding="utf-8")
        originals = analyze(reg, [src], only="graziani")
        check(not originals["errors"] and originals["scenarios"]["graziani"]["missing"] == 5,
              "original citations neither error nor satisfy the 2021 registry")
        check([e["citation"] for e in originals["original_citations"]] ==
              ["orig79:airlog:49.12", "orig79:land:4.22"], "original citations own ordered list")
        check(originals["original_citations"][1]["tested"], "original citation test locations")
        (src / "lib.rs").write_text("/// Cases: orig79:bogus:4.22, orig79:land:four, orig79:land:4.22:extra\nfn a() {}\n",
                                    encoding="utf-8")
        bad_originals = analyze(reg, [src])
        check(len(bad_originals["errors"]) == 3 and
              all(e["kind"] == "malformed_citation" for e in bad_originals["errors"]),
              "malformed original printing citations rejected")
        rep3 = analyze(reg, [root / "nope"])
        check(not rep3["inputs"]["crate_present"]
              and rep3["scenarios"]["graziani"]["missing"] == 5, "missing crate")
    print("self-test " + ("passed" if ok else "FAILED"))
    return 0 if ok else 1


# --------------------------------------------------------------------------- main

def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--scenario", help="report only this scenario (default: all in the registry)")
    ap.add_argument("--json", metavar="FILE", help="write the JSON report")
    ap.add_argument("--markdown", metavar="FILE", help="write the markdown report")
    ap.add_argument("--registry", type=Path, default=REPO / "data" / "rules")
    ap.add_argument("--src", type=Path, action="append",
                    help="engine source dir (repeatable; default crates/cna-rules/{src,tests})")
    ap.add_argument("--require-complete", action="store_true",
                    help="also fail when any applicable case is missing")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args(argv)
    if args.self_test:
        return self_test()
    src = args.src or [REPO / "crates" / "cna-rules" / "src",
                       REPO / "crates" / "cna-rules" / "tests"]
    try:
        rep = analyze(args.registry, src, args.scenario)
    except OSError as e:
        print(f"I/O error: {e}", file=sys.stderr)
        return 2
    if args.json:
        Path(args.json).write_text(json.dumps(rep, indent=2) + "\n", encoding="utf-8")
    if args.markdown:
        Path(args.markdown).write_text(markdown(rep) + "\n", encoding="utf-8")
    print(summary_text(rep))
    if rep["errors"]:
        return 1
    if args.require_complete and any(s["missing"] for s in rep["scenarios"].values()):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
