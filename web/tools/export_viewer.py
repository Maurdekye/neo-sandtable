"""Export registry paraphrases and authoritative coverage for the spectator build."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys
import tempfile
import tomllib

WEB = Path(__file__).resolve().parents[1]
REPO = WEB.parent
OUT = WEB / "src/data"


def main() -> None:
    OUT.mkdir(exist_ok=True)
    records = {}
    registry_digest = hashlib.sha256()
    for path in sorted((REPO / "data/rules").glob("*/*.toml")):
        raw = path.read_bytes()
        registry_digest.update(str(path.relative_to(REPO)).replace("\\", "/").encode())
        registry_digest.update(b"\\0")
        registry_digest.update(raw)
        document = tomllib.loads(raw.decode("utf-8"))
        section = document.get("section", {})
        book = section.get("book", path.parent.name)
        if section.get("id"):
            records[f"{book}:{section['id']}"] = {
                "title": section.get("title", ""),
                "summary": section.get("summary", ""),
                "timing": [],
            }
        for case in document.get("case", []):
            records[f"{book}:{case['id']}"] = {
                key: case.get(key, [] if key == "timing" else "")
                for key in ("title", "summary", "timing")
            }

    # A unique temporary folder permits independent test/build exports.
    with tempfile.TemporaryDirectory(dir=OUT, prefix=".coverage-") as folder:
        report_path = Path(folder) / "report.json"
        result = subprocess.run(
            [sys.executable, str(REPO / "tools/rules/coverage.py"),
             "--json", str(report_path)],
            capture_output=True,
            text=True,
        )
        if result.returncode:
            print(result.stdout + result.stderr, file=sys.stderr)
            result.check_returncode()
        report = json.loads(report_path.read_text(encoding="utf-8"))
    for scenario in report["scenarios"].values():
        scenario.pop("missing_ids", None)
        for row in scenario["by_anchor"]:
            row.pop("missing_ids", None)
    engine_digest = hashlib.sha256()
    for path in sorted((REPO / "crates/cna-rules").rglob("*.rs")):
        engine_digest.update(str(path.relative_to(REPO)).replace("\\", "/").encode())
        engine_digest.update(b"\\0")
        engine_digest.update(path.read_bytes())
    payload = {
        "schema": 1,
        "map_manifest": tomllib.loads(
            (REPO / "data/map/layers.toml").read_text(encoding="utf-8")),
        "map_corridor": tomllib.loads((REPO / "data/map/graziani-corridor.toml").read_text(encoding="utf-8")) if (REPO / "data/map/graziani-corridor.toml").exists() else {},
        "map_strips": tomllib.loads((REPO / "data/map/strips.toml").read_text(encoding="utf-8")) if (REPO / "data/map/strips.toml").exists() else {},
        "registry_sha256": registry_digest.hexdigest(),
        "engine_sha256": engine_digest.hexdigest(),
        "rules": records,
        "coverage": report["scenarios"],
        "inputs": report["inputs"],
        "warning_count": len(report["warnings"]),
    }
    # Publish complete JSON so a running Vite watcher never reads a partial export.
    with tempfile.NamedTemporaryFile(
        dir=OUT, prefix=".rules-", suffix=".json", mode="w",
        encoding="utf-8", delete=False,
    ) as temporary:
        temporary.write(json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n")
        temporary_path = Path(temporary.name)
    try:
        temporary_path.replace(OUT / "rules.json")
    finally:
        temporary_path.unlink(missing_ok=True)
    print(f"Viewer data: {len(records)} registry entries; {len(report['scenarios'])} coverage scenarios")


if __name__ == "__main__":
    main()
