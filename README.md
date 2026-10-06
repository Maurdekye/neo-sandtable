# neo-sandtable

A digital implementation of SPI's 1979 board wargame **The Campaign for North Africa** (CNA),
built to be played by AI agents — conventional LLMs and fast "System 1" models — as well as by
humans, while you watch the campaign unfold on a live board.

CNA is famous for its bookkeeping: fuel evaporation, water rations, truck loads, pilot
assignments, and the infamous Italian pasta rule. A computer can do all of that arithmetic.
What remains are the *decisions*, and those are what neo-sandtable puts in front of each
command seat.

> **Status: pre-alpha.** Nothing is playable yet. Work is under way on the engine skeleton, the
> live board, and the digitization of the first scenario. See [Roadmap](#roadmap).

## What it is (and isn't)

- **Faithful.** The rules engine implements the published mechanics under a pinned *rules
  profile*. An unresolved rule interaction stops the game with an explicit unsupported-case error;
  no model is ever allowed to invent an adjudication.
- **Seat-based.** Each side has five command seats — Commander, Logistics, Rear Area, Air,
  Front Line. Any seat can be driven by a human, an LLM agent (via the Claude Code, Codex, or
  Antigravity CLIs), a System 1 model (e.g. TypeSafe's Jev), or a scripted baseline, and seats can
  change hands mid-game.
- **Automated bookkeeping, explicit decisions.** The engine computes every cost, consumption, and
  table result. Players only answer genuine choices, through typed decision requests.
- **Live.** A WebGL board streams the campaign as the engine accepts orders, with playback,
  inspection, and per-seat perspectives.
- **Not a reproduction of the original components.** No scans, counter art, map art, or rules
  text from the published game are included. The map and counters are re-drawn as simplified SVG
  art from our own digitized data, and rules are implemented as code and structured data that
  cite the original case numbers. You will want a copy of the rules to follow along.

## Architecture at a glance

| Part | Technology | Location |
|---|---|---|
| Rules engine (pure, deterministic) | Rust | `crates/cna-core` |
| Content (map, units, tables, scenarios) | TOML / CSV data with rule citations | `data/` |
| Runner, persistence, live stream, MCP tools | Rust (SQLite per campaign) | `crates/` |
| Live board | TypeScript, React, PixiJS (WebGL) | `web/` |

See [`docs/architecture.md`](docs/architecture.md) for the full design, and
[`docs/decisions.md`](docs/decisions.md) for the decision log.

## Roadmap

1. **Vertical slice** — engine core, event log, scripted controllers, and a live board running a
   small synthetic scenario.
2. **Graziani's Offensive** (scenario 60.22, Game-Turns 1–6) with the full Land, Air, and Logistics
   systems, played AI-vs-AI while you watch.
3. **LLM and System 1 seats** — Claude Code, Codex, and Antigravity via MCP tools; Jev via its API.
4. **Human order entry** for every decision type.
5. **The Italian Campaign** (60.23), then the later scenario groups, and eventually the full
   111-turn campaign.

## Building

Requirements: Rust (stable, pinned via `rust-toolchain.toml`) and Node.js 22+.

```sh
cargo test --workspace
```

The web board lives in `web/` (setup instructions will appear there once it exists).

## Acknowledgements and legal

*The Campaign for North Africa* was designed by Richard Berg and published by Simulations
Publications, Inc. in 1979. This is an unofficial fan project with no affiliation with the game's
designers, publishers, or current rights holders. Game mechanics are implemented independently;
no original art, scans, or rules text are distributed.

Code is released under the [MIT License](LICENSE).
