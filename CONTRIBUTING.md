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

The measurable bar: no run of **10 or more consecutive words** that also occurs in a rulebook,
unless the run is an official name (a phase, segment, table, unit or place). Chart legends follow
the same rule. `tools/content/check_verbatim.py` finds such runs; run it before pushing anything
that contains prose (summaries, notes, footnotes, interpretations, READMEs):

```sh
python tools/content/check_verbatim.py            # all of data/ and docs/; exit 1 on a prose run
python tools/content/check_verbatim.py data/rules/airlog
```

It needs `CNA_SOURCES`, so it cannot run in CI; passing it is each author's responsibility.

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
- **Landing lock.** Pushes to `main` are serialized by a lock on the remote, the
  `landing-lock` branch, managed by `tools/land.py` with a fair queue:
  ```sh
  git pull --rebase origin main && <run the checks>   # before queuing, on a fresh main
  python tools/land.py acquire --who <your-name>      # queues you; returns at your turn
  git pull --rebase origin main      # "up to date": main is what you checked, push now;
                                     # otherwise re-run the checks first
  git push origin HEAD:main
  python tools/land.py release --who <your-name>      # always, even if the push failed
  ```
  Waiters are served oldest ticket first. Re-running `acquire` keeps your place, so a wait that
  times out, or a tool call that is killed, loses nothing; just run it again. A queued owner who
  does not take a free lock within 90 seconds is skipped. Hold the lock only for rebase, checks
  and push, never while editing. A lock older than 25 minutes counts as abandoned; if your checks
  run long, `python tools/land.py refresh --who <your-name>` restarts that clock. `status` shows
  the holder and the queue, `leave` gives up your place. Never push to `main` while someone else
  holds the lock, and never force-push `main`; `tools/land.py` force-updates only its own
  `landing-lock` and `landing-queue/*` refs, with leases.
- **Test time.** The checks everyone runs before each push must stay quick, so the landing
  queue keeps moving. Each test in the default `cargo test --workspace` should finish within
  about 60 seconds on this shared, busy machine. Slower end-to-end tests (whole campaigns with
  active movers, the launcher's scripted campaign) are marked `#[ignore = "slow: <why>"]` and run
  in CI's `rust-slow` job after every push. Run them yourself with
  `cargo test -p <crate> -- --ignored` when you change what they cover, and keep a bounded
  version (one game-turn, a fixed number of decisions) in the default set, so the same paths
  are still exercised before each push. Never just raise a timeout to make a slow test fit.
  A slow test's own limit is set to about twice its measured duration on the CI runner, with
  that measurement in a comment beside the limit, so it still catches hangs and regressions.
- **Disk.** All agents build on one machine. Keep a single clone, build with the workspace
  profile (small debug info, no incremental cache), and run `cargo clean` in your clone if its
  `target/` grows past a few GB.
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

Two kinds of change run the content checks instead:

```sh
python tools/content/check_verbatim.py
python tools/content/check_utf8.py
```

- Markdown under `docs/` and nothing else.
- A standalone script under `tools/` that no build target and no CI job reads. Check before you
  claim it: `grep -rn <script> --include=*.rs --include=*.ts .github/` must come back empty.
  `tools/content/` and `tools/rules/coverage.py` do **not** qualify — CI runs them.

Neither can affect the Rust or web gates, and the landing lock is shared: making everyone else wait
through a full matrix for a prose edit costs the queue more than it protects. Any other change —
`data/`, a chart, a schema, one line of Rust or TypeScript — runs the whole list above. If you are
unsure which case you are in, you are in the last one.

Say in the commit message which checks you ran. Never describe a gate you skipped as passing.

### Measuring anything across two commits

Two traps have already invalidated real measurements here. Read this before you compare timings,
counts or outputs between revisions.

**A shared target directory can make two commits run the same data.** `cna_content::repo_data_dir`
resolves through `env!("CARGO_MANIFEST_DIR")`, which is baked in when the crate is compiled. If two
checkouts share a `CARGO_TARGET_DIR`, the second may reuse the first's binary and read the first's
`data/` — so the two "different heads" are reading identical content and the comparison means
nothing. Use a fresh target directory per head, and prove it: hash each test binary and check the
data root it actually compiled against before you believe a single number.

**Wall-clock numbers taken under load measure the load.** Other agents land continuously, so a
`cargo` run competes with their builds and tests. Before attributing a slowdown to a code or data
change, measure in a quiet window and say what else was running. A timing assertion that only holds
on an idle machine does not belong in a gate; put it where a clock means something, or set its
threshold for the loaded case. Record the isolated time, the loaded time and the relevant data sizes
beside any threshold you choose, so the next person can recompute the ratio instead of rediscovering
this paragraph.

And ratios of *bounds* are not ratios of *work*: a cost growing faster than every input dimension is
worth investigating, but a published bound is an upper limit, not a measurement of what ran.

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
| Typed table bindings | `crates/cna-tables/` |
| Units, organization, equipment | `data/units/` |
| Scenarios | `data/scenarios/` |
| Live board | `web/` |
| Project docs and decisions | `docs/`, top-level files, `tools/content/` (lead agent) |

## 5. Determinism and units

- Engine code must be deterministic: no wall-clock reads, no hash-order iteration in anything that
  affects results (use `BTreeMap`/`IndexMap` or explicit sorting), no floating point in rules
  arithmetic. Randomness comes only from the campaign RNG passed into a transition.
- Every quantity carries its unit, in the type (Rust newtypes) or in the field name/schema (data).
- Identity never rests on display names. Use stable ids.
