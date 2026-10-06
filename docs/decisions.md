# Decision log

Decisions that shape the project, newest last. **O** = decided by the project owner;
**L** = default set by the lead agent, open to the owner's override. Where this log conflicts with
[`design-v0.1.md`](design-v0.1.md), this log wins.

## D1 (O) — Start from scratch
No code is reused, ported, or rewritten from existing CNA digitization projects (in particular
Sandtable). neo-sandtable is a spiritual successor, built independently.

## D2 (O) — First target: Graziani's Offensive, full systems
The first complete, faithful playable game is **Graziani's Offensive** (`scen:60.22`: Game-Turn 1,
OpStage 1 through the end of Game-Turn 6, OpStage 3) played with the **full Land, Air, and
Logistics games**. None of the official abstractions (`land:32`, `airlog:47`, `airlog:58`) are
used. Every rule case that can arise in that window must work; cases that cannot arise there (for
example German units, `land:31` Rommel) are classified as not applicable to this scenario.
**The Italian Campaign** (`scen:60.23`, Game-Turns 1–20) follows.

## D3 (O) — Rules baseline
The authoritative text is the July 2021 retype of the rulebooks with integrated addenda and errata
(Land, Air & Logistics, Scenarios). Where it appears to differ from the 1979 printing, the
difference is checked against the original and recorded as an interpretation.

## D4 (O) — Interpretations
The development team rules on ambiguities and records each ruling in
[`interpretations/`](interpretations/). The owner reviews consequential rulings in batches and may
overturn any; an overturned ruling creates a new rules-profile version and never rewrites an
existing campaign.

## D5 (O) — Publication and assets
The project is published as this public repository. The original game's assets are not reused:
the map and counters are recreated as simplified SVG art from our own digitized data. No scans,
original artwork, or verbatim rules text are committed (see `CONTRIBUTING.md` §1).

## D6 (O) — Priority: AI spectating first
Order of work: live board and scripted baseline controllers; then LLM and System 1 seats playing
the first scenario end to end while a spectator watches; then complete human order entry. Human
play remains a requirement — it is sequenced after AI-vs-AI play, not dropped.

## D7 (O) — AI seats use subscription CLIs
Conventional-LLM seats drive the **Claude Code, Codex, and Antigravity** CLIs. Other language
servers and APIs (OpenRouter, Ollama, …) come later; the adapter design must leave room for them.
System 1 seats use TypeSafe's **Jev** API. No paid run starts without an explicit per-run budget, and
every run carries limits on concurrent sessions, calls, and elapsed time.

## D8 (O) — Technology
- **Rust** for the rules engine, runner, persistence, MCP server, and controller adapters.
- **TypeScript + React** for the web board, with **PixiJS (WebGL)** rendering. Types shared from
  Rust via generated TypeScript.
- **MCP tool server** as the interface for LLM seats: seat-scoped tools to observe, list legal
  actions, validate, submit, message the team, and keep notes.
- **One long-lived CLI session per LLM seat** for the whole campaign, relying on the CLI's own
  compaction. Durable seat notebooks are still kept by the game, so handover and recovery never
  depend on a session's memory.
- **Rules encoded as typed code plus data**: procedures are Rust code citing case numbers; every
  chart and table is a cited data file; a rule-case registry drives coverage and the decision
  catalogue.
- **MIT license.**
- **Commits go directly to `main`**; CI runs on every push.

## D9 (L) — Engineering defaults
- One SQLite file per campaign: append-only event log, accepted commands (with idempotency keys),
  periodic checkpoints. An accepted order, its events, the RNG state and pending decisions commit in
  one transaction.
- One local server process, listening on localhost only. Remote play comes later.
- A seeded, versioned RNG; the game's dice are emulated exactly, including two-dice "11–66"
  readings where the rules use them.
- Axial hex coordinates internally across all five map sections; display ids in the game's own form
  (`C4218`).
- Hand-maintained data in TOML/CSV, every record citing its source; JSON generated for the board.
- Tests: unit, snapshot, property-based, and replay-equality tests; CI via GitHub Actions.
- Counter art uses NATO-style unit symbols on flat-coloured counters; terrain art uses flat fills
  and hexside lines. All generated from data.
- Default AI run limits: at most 2 concurrent CLI sessions per run, plus caps on calls and elapsed
  time.
- Player visibility follows `land:3.6` (Limited Intelligence): stack presence on the map is public;
  status and composition of enemy units are hidden unless a rule says otherwise. The local operator
  gets a clearly labelled omniscient view; AI seats only ever receive their own filtered view.
- Team communication: free structured messages within a team, none across teams; the commander
  arbitrates shared resources.
