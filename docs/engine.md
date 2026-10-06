# The CNA engine: how to add rules

This guide is for anyone implementing rule procedures in `crates/cna-rules`. Read
[`architecture.md`](architecture.md) first for the overall design.

## 1. Where things live

| Crate | What it gives you |
|---|---|
| `cna-core` | The contract: `Ruleset`, `evaluate`, decisions (`DecisionRequest`, `ActionSpace`), audience-tagged events, `Perspective`, dice (`cx.rng`), quantity newtypes, hex geometry. |
| `cna-content` | Typed data: `MapContent` (hexes, terrain, aliases), `UnitsContent` (weapons, classes, aircraft, OA sheets, schedules), `ScenarioContent` (set-up, supply, facilities, victory), `Registry` (every rule case). |
| `cna-tables` | Typed lookups for every chart and table, each citing its case. Never hard-code a table value. |
| `cna-rules` | The CNA ruleset: `CnaContent` (everything above, loaded for one scenario), `State`, the sequence of play (`seq`), step procedures (`steps`), and the views (`view`). |

## 2. How a campaign runs

`State.cursor` walks the sequence of play in `seq.rs`, one **step** at a time. Every step is named
by its timing anchor from `data/rules/README.md` (`opstage.movement_and_combat.movement`). For
each step the engine:

1. **Enters** the step: emits `PhaseChanged` and calls `Cna::enter_step`, which dispatches on the
   anchor to the step's procedure. Automatic procedures just change state and emit events.
   Procedures that need a player opens one or more decisions (`steps::open`).
2. Waits while decisions are pending. Each answer goes through `Cna::respond`, which checks
   the id, seat and revision, then calls `Cna::respond_to`, which dispatches on the decision
   `kind`. A handler may open further decisions, for example the next unit to move.
3. Moves to the next step when the step has been entered and nothing is pending. Steps belonging
   to a system the scenario doesn't use (Air, Logistics) are skipped.

Player A runs phases G–M, then Player B (`seq::PLAYER_HALF`). Player A may repeat the
Movement-and-Combat segments (`land:8.2`): call `state.cursor.repeat_movement_and_combat()` from
the reserve-release handler when the player asks for another cycle.

**Profiles.** `Cna::dev()` (`cna-2021-dev`) skips steps that have no procedure yet, so a
campaign can be watched end to end while rules are filled in. `Cna::full()` (`cna-2021-full`)
stops with `EngineError::Unsupported { case }` at the first step whose applicable procedural
cases are not implemented. Write every procedure so that it is correct under `full`.

## 3. Adding a procedure, step by step

1. **Pick the step.** `python tools/rules/coverage.py --scenario graziani --markdown out.md`
   lists, per anchor, the applicable cases and which are still missing. Read those cases in the
   registry (`data/rules/<book>/NN-*.toml`) and in the rules text.
2. **Write the procedure** in the module of its rules area: `src/land/<topic>.rs`,
   `src/logistics/<topic>.rs`, `src/air/<topic>.rs`. Create the module if needed. Keep
   `steps.rs` as the dispatcher: one match arm per anchor in `enter_step`, one per decision kind
   in `respond_to`.
3. **Cite the cases.** Every procedure's doc comment ends with a line
   `/// Cases: land:8.31, land:8.32` (several lines are fine). The coverage tool counts a case as
   implemented when it is cited there, and as tested when a test in a `#[cfg(test)]` module also
   cites it on a `/// Cases:` line. Cite an interpretation you implement as `interp:land-0001`.
   If a case cannot be supported in a profile, say so with
   `/// Unsupported: land:x.y — reason` and return `EngineError::Unsupported` when it is reached.
4. **Extend the state** in the sub-state of your area (`LandState`, `LogisticsState`,
   `AirState`, or a new one). Keep it plain data in `BTreeMap`s. Never store anything derivable
   from content, such as class ratings, terrain or table values. Look those up by id.
5. **Use the tables** through `content.tables` (crate `cna-tables`). If a table you need has no
   binding yet, ask its owner (Land: rules-land, Air and Logistics: rules-airlog). Don't parse
   TOML in `cna-rules`.
6. **Open decisions** with `steps::open(state, cx, seat, kind, summary, rules, trigger, secrecy,
   space)`:
   - `seat`: the owning role from the registry's `seat` field for that case; `either` means the
     seat that owns the affected unit. Use `SeatId::new(side, Role::X)`.
   - `kind`: a stable string `cna.<topic>.<what>`; `respond_to` dispatches on it.
   - `space`: the legal action space as an `ActionSchema`. Make it enumerate exactly the legal
     choices when practical (units that can still move, hexes they can reach), so scripted and AI
     seats can act without guessing. Offer an explicit "done" option, or `with_pass`, for as long
     as the seat may keep acting, and re-open the next decision from the handler.
   - The `summary` and `ChoiceOption` labels and details are what an AI seat reads. Write them in
     plain English, in our own words.
7. **Reject illegal answers** with `steps::illegal(message)` before changing anything. `respond`
   runs on a cloned state, so an error leaves no trace, but keep the habit. A message must never
   reveal hidden information: say "not a legal destination", not "enemy unit at C4020".
8. **Randomness** comes only from `cx.rng` (`d6()`, `two_dice_reading()`). Emit a `DiceRolled`
   event citing the rule for every roll.
9. **Events and secrecy** (`land:3.6`): facts about a side's own units go to `Audience::Side(side)`.
   Public facts (stack presence, weather, combat totals) go to `Audience::Public`. When the enemy
   sees a redacted version of something the owner sees in full, emit the full version to
   `Audience::Side(owner)` and the redacted copy to `Audience::SideOnly(enemy)`. That keeps the
   operator from receiving both.
10. **Test it** in the module's `#[cfg(test)]` block, on the real Graziani content
    (`CnaContent::load(&cna_content::repo_data_dir(), "graziani")`; see `src/tests.rs` for a
    whole-campaign harness) or on a small hand-built state. Cite the cases on a `/// Cases:`
    line in the test's doc comment. At minimum, test one legal path, one rejection, and that
    the enemy perspective learns nothing it shouldn't.

## 4. Conventions

- **Determinism:** no wall clock, no hash-order iteration, no floats in rules arithmetic.
  Rounding is a named function citing the rule that defines it.
- **Quantities** carry their unit (`cna_core::quantity`). Propose new newtypes to the lead.
- **Ids** are stable strings from the data: unit ids from the OA sheets, hex ids canonicalized
  with `content.map.canonical`. Never use a display name as an id.
- **Content rules** (CONTRIBUTING §1) apply to code too: no rules prose in comments, summaries
  or labels. Paraphrase and cite.
- **One procedure, one owner:** the area owners are in `docs/ownership.md`. Changes to `seq.rs`,
  `state.rs` (shared fields), `lib.rs` and `view.rs` go through the lead.

## 5. What exists so far

- Sequence of play for the full Land + Air + Logistics game, with skipping by system.
- Initial state from the scenario: every deployed unit placed or awaiting its owner's set-up
  choice, the rest not yet arrived; dumps, dummy dumps, truck pools, air forces including Malta.
- Initiative (`land:7`): the scenario fixes Game-Turn 1, later turns are rolled; each OpStage
  the holder declares Player A or B.
- Views: board view, `observe`, `inspect`, all filtered by `land:3.6`.
- End of game: reported without victory determination (not implemented yet).

The first open procedures, in sequence order: scenario set-up placement (`setup`: units and
dumps with area placements, first-line truck distribution, `scen:59`), weather (`land:29`),
organization (`land:19`), and movement with capability points, stacking and zones of control
(`land:6`, `8`, `9`, `10`).
