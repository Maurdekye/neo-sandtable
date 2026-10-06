# Architecture

This is the working architecture. It refines [`design-v0.1.md`](design-v0.1.md) under the
decisions in [`decisions.md`](decisions.md). When code and this document disagree, fix one of them.

## 1. Shape of the system

```text
                +---------------------------- cna-server (one process, localhost) ------------------------+
 web board  <-- | HTTP + WebSocket API  <-- viewer projections <-- event log + checkpoints (SQLite)      |
 (React +    -->|        |                                               ^                                |
  PixiJS)       |        v                                               |                                |
                |   campaign runner (single writer) --> cna-core::evaluate(world, content, profile, cmd) |
                |        ^         |                                                                      |
                |        |         v                                                                      |
                |   controller adapters <-- seat observations (filtered per rules visibility)            |
                |   - scripted baselines (in-process)                                                     |
                |   - MCP tool server  <---- Claude Code / Codex / Antigravity CLI sessions (one per seat) |
                |   - Jev client (System 1)                                                               |
                +-----------------------------------------------------------------------------------------+
```

Every controller type, from a human clicking on the board to an LLM calling an MCP tool, ends at
the same place: a `DecisionResponse` submitted to the runner, which hands one command at a time to
the pure engine.

## 2. Crates

| Crate | Role | Depends on |
|---|---|---|
| `cna-core` | Pure, deterministic engine: ids, quantities, hex geometry, world state, commands, events, decisions, RNG/dice, phase machine, `evaluate`. No I/O. | — |
| `cna-content` | Loads `data/` into typed, validated, hashed content packages (map, units, tables, scenarios, rule-case registry). | `cna-core` |
| `cna-rules` *(when it outgrows core)* | Rule procedures grouped by rules section, each citing its cases. | `cna-core` |
| `cna-server` | Runner, persistence (SQLite), HTTP/WebSocket API, viewer projections, MCP tool server, controller adapters. | all of the above |
| `cna-cli` *(later)* | Headless runs, data validation, coverage reports. | all |

Rust types that cross into the board are exported to TypeScript with `ts-rs` into
`web/src/generated/`. The board never hand-writes a type the engine owns.

## 3. Core model

### 3.1 Static content vs. dynamic state

- **Content** (immutable for a campaign, from `data/`): map, terrain effects, unit and equipment
  characteristics, organization (OA) charts, all charts and tables, scenario setups, reinforcement
  schedules, the rule-case registry. Each content package has a hash pinned by the campaign.
- **World** (the authoritative dynamic state): clock, formations and their composition, supplies,
  trucks and cargo, air assets, projects, weather, knowledge records, pending decisions, RNG state.
- **Rules profile**: which systems are on (Land, Air, Logistics, and any official abstraction),
  errata/interpretation set, information profile, assistance profile.

### 3.2 The clock

The game clock mirrors the published sequence of play (`airlog:33.0` for Land+Air, `airlog:48.0`
for Logistics; `land:5.2` for Land alone):

```text
GameTurn (1..111, one week)
  Stage: Initiative · Strategic Air Planning · Naval Convoy · Logistics stages ·
         OpStage 1 · OpStage 2 · OpStage 3 · Strategic Air Recovery
    Phase (lettered A..M within a stage; OpStage phases G..M run once for Player A, then for B)
      Segment (numbered)  — e.g. Movement · Breakdown · Combat · Reserve Release
        Step (lettered)   — e.g. Position · Barrage · Retreat Before Assault · Force Assignment · …
  + repeatable cycles (Movement-and-Combat segments repeat under continual movement, land:8.2)
```

The exact sequence is data-driven per rules profile and implemented as an explicit state machine;
it is never inferred.

### 3.3 Commands, events, decisions

```text
evaluate(&World, &Content, &Profile, Command, &mut Rng)
    -> Ok(Transition { world', events: Vec<Event>, pending: Vec<DecisionWindow> })
    -> Err(Rejection)            // no mutation, no RNG advance
```

- **Command**: an order from a seat, or an internal `Advance` the runner issues to run automatic
  steps. Commands are validated against the current decision window.
- **Event**: an immutable record of what happened (unit moved, dice rolled, fuel consumed, combat
  resolved). Every event carries a **visibility tag** saying which sides/seats may see it and at
  what detail, so projections can filter without re-deriving rules.
- **DecisionWindow**: who must decide what, by when in the sequence, with what legal action space,
  and how the window resolves (single seat, simultaneous secret submissions, ordered sequence).
  Interrupts (reactions, triggered decisions) push a window and store a continuation.

The runner loop: issue `Advance` until a window opens → dispatch decision requests to the owning
seats' controllers → accept responses one at a time through `evaluate` → persist → stream →
repeat.

### 3.4 Determinism

- RNG: a seeded, versioned generator (`rng-v1` = ChaCha8). The RNG state is part of the world and of
  every checkpoint. Draws happen only inside accepted transitions.
- Dice: CNA uses d6 rolls; some tables read two dice as tens and units (11–66). The dice module
  emulates exactly the rolls the rules call for and records each roll in an event.
- No floating point in rules arithmetic: integer quantities, explicit rounding functions named after
  the rule that defines them.
- Stable iteration order everywhere (`BTreeMap`, sorted ids).
- Replay of accepted commands with recorded randomness reproduces identical events and state
  hashes (a CI test).

### 3.5 Quantities and ids

Every quantity is a newtype with its unit: `CapabilityPoints`, `FuelPoints`, `AmmoPoints`,
`StoresPoints`, `WaterPoints`, `Tons`, `TruckPoints`, `TOEStrengthPoints`, … Arithmetic between
different units does not compile. Ids are stable strings or interned integers, never display names.

### 3.6 Hex geometry

The five map sections A–E sit side by side west to east. The `01xx` hex-row of each section
overlays the `39xx` hex-row of the section to its west (`land:4.1`), so seam hexes have two printed
ids. Internally the whole map is one axial coordinate space; `HexId` converts both ways and maps
seam aliases to one canonical hex. The exact geometry (orientation, stagger, row/column meaning) is
pinned in `data/map/README.md` by the map digitization work.

## 4. Visibility

Three layers, never mixed:

1. **World**: full truth, only for adjudication and the explicitly authorized omniscient view.
2. **Seat observation**: what a seat may legitimately know under the information profile. Baseline
   is `land:3.6` Limited Intelligence: stack presence on the map is public; status, composition and
   attributes of enemy units are hidden unless a rule discloses them (for example combat totals).
3. **Viewer projection**: a seat's view, a side's view, or the operator's omniscient view (clearly
   labelled).

Filtering happens on the server for every channel: snapshots, event streams, MCP tool results,
validation errors, logs and exports.

## 5. Seats and controllers

- Ten default seats: Commander, Logistics, Rear Area, Air, Front Line for each side. Every actionable
  entity or decision domain has exactly one owning seat (configurable ownership map).
- A controller binding = seat + controller kind (`scripted`, `llm-cli`, `system1`, `human`) +
  configuration + `controllerEpoch`. Handover increments the epoch; responses carrying an old epoch
  are rejected.
- **LLM seats (decision D8):** one long-lived CLI session per seat for the whole campaign (Claude
  Code, Codex, or Antigravity). The session reaches the game through the **MCP tool server**:

  | Tool | Purpose |
  |---|---|
  | `observe` | The seat's filtered situation report and pending decisions |
  | `inspect` | Authorized details of a hex, stack, unit, dump, airfield, … |
  | `describe_actions` | The legal action space for a pending decision, with parameter domains |
  | `validate` | Check a draft order without committing (no randomness, no leaks) |
  | `submit` | Commit a decision response (idempotent, epoch-checked) |
  | `message_team` / `read_messages` | Structured team coordination |
  | `notebook_read` / `notebook_write` | Durable seat notes kept by the game, not the session |

  Each seat gets its own endpoint and token; tools are scoped to that seat's permissions.
- **System 1 seats** answer bounded, typed questions generated from the same action spaces; the
  adapter turns their choices into ordinary commands.
- **Scripted baselines** (legal-random, conservative, simple objective-driven) run in-process and
  come first.
- Failure handling: a timeout, invalid answer, outage or exhausted budget pauses the decision and
  offers retry or takeover (or a declared fallback). It never becomes a pass, retreat or surrender.

## 6. Persistence

One SQLite database per campaign:

| Table | Contents |
|---|---|
| `campaign` | Pins (rules profile, content hashes, engine and RNG versions), lifecycle |
| `commands` | Accepted commands with idempotency keys (unique), seat, epoch, sequence |
| `events` | Append-only, sequenced, with visibility tags |
| `checkpoints` | Periodic serialized world snapshots (including RNG state) |
| `decisions` | Open and resolved decision windows, secret submissions |
| `seats` | Controller bindings and epochs |
| `notebooks`, `messages` | Durable seat notes and team messages |

An accepted command, its events, the new pending decisions, the RNG state and the revision commit in
one transaction; clients are acknowledged after the commit.

## 7. Live board protocol

- HTTP for commands and queries; one WebSocket per viewer for the event stream.
- A subscriber receives an authorized **snapshot at sequence N**, then every event after N, each
  with its sequence number. Gaps trigger a resync from a newer snapshot. Slow viewers are dropped
  back to snapshots; they never block the runner.
- Simulation speed and animation speed are independent. "Pause playback" (viewer-local) and "Pause
  campaign" (runner, at a safe boundary) are different controls. Historical frames are read-only.

## 8. Rendering (board)

React for panels (inspector, event feed, seats, timeline); PixiJS (WebGL) for the map:

- terrain layer built from `data/map` (fills, hexside lines, features, labels), cached per zoom band;
- counters drawn from our own NATO-style SVG designs, rasterized to textures and batched;
- overlays (supply, transport, air missions, projects, control) as separate layers;
- dense stacks remain selectable (stack fan-out on hover/selection).

## 9. Rule coverage

`data/rules/` holds the **rule-case registry**: every case of the baseline rulebooks with its
disposition — automatic, player decision (owner seat + decision schema), data constraint,
display/disclosure, superseded by errata, not applicable to a scenario, or unresolved. Code
procedures cite their cases; a coverage tool cross-checks the registry against the code and
reports, per scenario, which applicable cases are implemented, tested, or still missing. A
scenario is playable under a profile only when every applicable case is implemented or explicitly
marked unsupported (which stops the game if reached).
