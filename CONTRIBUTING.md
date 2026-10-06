# Contributing

This project is built mostly by AI agents working in parallel, coordinated by a lead agent, under
decisions made by the project owner. These rules keep parallel work from colliding and keep the
repository legally clean. They apply to humans too.

## 1. Content rules (non-negotiable)

The original game's components are copyrighted. The repository must never contain:

- scans or photos of the rulebooks, charts, map, or counters;
- the original map or counter artwork, or images derived from them by tracing, filtering or
  re-colouring (including anything from the VASSAL module);
- verbatim rules prose. Paraphrase in your own words. Short identifiers are fine: case numbers
  (`[8.37]`), section titles ("Breakdown"), unit designations, place names.

What *is* allowed, and expected:

- **Game data as structured records**: terrain per hex, unit values, chart and table numbers,
  scenario setups — each record citing the rule case it comes from.
- **Our own art**: simplified SVG map and counter graphics generated from that data.
- **Paraphrased notes** explaining how a rule was implemented.

The digitization sources (the VASSAL module, its retyped rulebooks, the 1979 scans) live
**outside** the repository. Tools that read them take the location from the `CNA_SOURCES`
environment variable and must never copy source files into the tree. `.gitignore` blocks the usual
suspects (`*.vmod`, `/local-sources/`), but you are responsible for what you commit.

## 2. Citations

Every rules-derived record and every rules procedure in code cites its source:

| Code | Source |
|---|---|
| `land:<case>` | Land Game rulebook, July 2021 errata-integrated retype (baseline text) |
| `airlog:<case>` | Air & Logistics Games rulebook, July 2021 retype |
| `scen:<case>` | Scenarios booklet, July 2021 consolidated corrections |
| `errata79:<case>` | September 1979 addenda, where cited separately |
| `orig79:<book>:<case>` | The 1979 printing, only when it differs from the retype |
| `interp:<id>` | A recorded interpretation (see `docs/interpretations/`) |

Examples: `land:8.37`, `airlog:49.1`, `scen:60.22`. In Rust, procedures carry a doc comment or
attribute naming their cases, so the coverage tooling can find them.

When the rules are ambiguous or contradictory, do not guess silently. Record an interpretation
(`docs/interpretations/`) with the cases involved, the conflict, the choice made and why, and the
tests that pin it. The lead agent batches consequential interpretations for the owner's review.

## 3. Git workflow

- **Commit directly to `main`** (owner's decision). There are no long-lived branches.
- Work in **your own clone**, never in someone else's working tree.
- Before every push: `git pull --rebase origin main`, re-run the checks, then push. Never
  force-push `main`. If a rebase conflicts in a file you do not own, stop and ask the owner of that
  area (see §4) instead of resolving it yourself.
- Keep commits small and focused, with a prefix naming the area:
  `core: …`, `rules: …`, `map: …`, `units: …`, `scenario: …`, `server: …`, `web: …`, `docs: …`,
  `ci: …`, `tools: …`.
- CI must stay green. Do not push code that fails the checks below.
- Lockfiles (`Cargo.lock`, `web/package-lock.json`) are generated: on a rebase conflict, take
  either side and regenerate them mechanically (`cargo check`, `npm install`); no need to ask.

### Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# web/, once it exists:
npm --prefix web ci && npm --prefix web run lint && npm --prefix web run test && npm --prefix web run build
```

## 4. Areas and owners

Each area has one owning agent at a time, recorded in `docs/ownership.md`. You may read anything,
but edit outside your area only for small, obviously-needed fixes, and say so in the commit
message. Cross-area changes (a shared schema, a core type) go through the lead agent.

| Area | Paths |
|---|---|
| Engine core | `crates/cna-core/` |
| Content loading and validation | `crates/cna-content/` |
| Server, runner, persistence, MCP | `crates/cna-server/` and related crates |
| Map data and map tooling | `data/map/`, `tools/map/` |
| Rules case registry and tables | `data/rules/`, `data/tables/` |
| Units, organization, equipment | `data/units/` |
| Scenarios | `data/scenarios/` |
| Live board | `web/` |
| Project docs and decisions | `docs/`, top-level files (lead agent) |

## 5. Determinism and units

- Engine code must be deterministic: no wall-clock reads, no hash-order iteration in anything that
  affects results (use `BTreeMap`/`IndexMap` or explicit sorting), no floating point in rules
  arithmetic. Randomness comes only from the campaign RNG passed into a transition.
- Every quantity carries its unit, in the type (Rust newtypes) or in the field name/schema (data).
- Identity never rests on display names. Use stable ids.
