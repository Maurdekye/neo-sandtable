# Rule-case registry

Every numbered case of the baseline rulebooks (July 2021 retype; decision D3) appears here
**exactly once**, with what the engine must do about it. The registry is the backbone of the
decision catalogue (which choices reach players, and who makes them) and of coverage reports
(which applicable cases are implemented and tested).

Schema owner: the lead agent. Propose changes to the lead; do not fork the schema per book.

## Files

```
data/rules/land/NN-<section-slug>.toml      # Land rulebook, one file per major section (01..32, addenda)
data/rules/airlog/NN-<section-slug>.toml    # Air & Logistics rulebook (33..58)
data/rules/scen/NN-<section-slug>.toml      # Scenarios booklet (59..65)
```

`NN` is the zero-padded section number (`08-land-movement.toml`). Cases appear in book order.

## Record format

```toml
[section]
id = "8.0"
book = "land"                 # land | airlog | scen
title = "Land Movement"
systems = ["land"]            # land | air | logistics | naval — which game systems it belongs to

[[case]]
id = "8.37"                   # exactly as printed
title = "Terrain Effects Chart"   # short identifier; "" if the case has no title
summary = "Lists the movement cost and combat effects of each terrain and hexside type."
                              # 1–3 sentences IN OUR OWN WORDS (never copied text)
kind = "table"                # rule | procedure | definition | table | example | commentary | designer_note | addendum
disposition = "data"          # see below
seat = "none"                 # owner of the decision (see below); "none" unless disposition = "decision"
timing = ["opstage.movement_and_combat.movement"]   # sequence-of-play anchors (vocabulary below)
tables = ["land.8.37.terrain_effects"]              # ids in data/tables, if any
depends_on = ["8.2", "8.31"]  # other cases this one needs (same book unless prefixed "airlog:"…)
errata = ["errata79:8.37"]    # errata that touched this case, if any
interp = []                   # interpretation ids (docs/interpretations), if any
src = ["land:8.37"]

[case.applies]
graziani = "yes"              # yes | no | conditional — does it arise in scen:60.22 with full systems?
graziani_note = ""            # required for "no" and "conditional": why

# Only when disposition = "decision":
[case.decision]
what = "Choose barrage targets for each artillery unit in position."
params = ["firing unit", "target hex", "barrage points allotted"]
trigger = "scheduled"         # scheduled (at a phase) | triggered (by an event) | standing (policy)
secrecy = "secret_simultaneous"   # open | secret | secret_simultaneous
interrupts = false            # true if it suspends another action (e.g. reaction)
```

### `disposition`

| Value | Meaning |
|---|---|
| `automatic` | The engine performs it with no player input (costs, consumption, table results, bookkeeping). |
| `decision` | A player choice. Needs `seat`, `timing`, and a `[case.decision]` table. |
| `data` | Its content is data (a table, a unit value, a scenario constraint) captured in `data/`. |
| `display` | An information-disclosure or display requirement (what a side may see). |
| `superseded` | Replaced by errata; cite it in `errata`. |
| `none` | No engine effect (commentary, example, designer note, cross-reference). |
| `unresolved` | Ambiguous or unsupported for now; must have an `interp` entry or a GAPS.md note. |

A case that mixes several (a procedure with a decision inside) gets the disposition of its most
demanding part (`decision` > `automatic` > `display` > `data` > `none`) and explains the rest in
its summary. Split nothing; the case number is the unit.

### `seat`

`commander`, `front_line`, `rear_area`, `logistics`, `air`, `naval` (the seat holding the naval
domain; by default the commander), `either` (whichever seat owns the affected unit or asset), or
`none`. These are default owners; campaigns may remap them.

### `timing` vocabulary

Dotted paths following the sequence of play (`airlog:33.0` Land/Air, `airlog:48.0` Logistics,
`land:5.2` Land alone). Use the most specific level that applies; use several anchors if a case
applies in several places.

```
setup                                  # scenario set-up only
initiative                             # Stage I
strategic_air.designation | .malta_availability | .mission_assignment | .malta_raid
naval_convoy.schedule | .recon | .lane_assignment | .bombing
logistics.<phase>                      # Logistics-game stages (extend from airlog:48.0; tell the lead)
opstage.initiative_declaration
opstage.weather
opstage.organization.reorganization | .construction | .training | .supply_distribution | .tactical_shipping
opstage.convoy_arrival
opstage.cw_fleet.assignment | .repair
opstage.land_support_air.assignment | .deployment | .air_combat | .flak | .completion | .return | .maintenance
opstage.reserve_designation
opstage.movement_and_combat.movement
opstage.movement_and_combat.breakdown
opstage.movement_and_combat.combat.position | .barrage | .retreat_before_assault | .force_assignment | .anti_armor | .close_assault
opstage.movement_and_combat.reserve_release
opstage.truck_convoy_movement
opstage.cw_rail_movement
opstage.repair.towing | .maintenance
opstage.patrol
strategic_air_recovery.<phase>
end_of_turn
continuous                             # a standing constraint checked whenever relevant (e.g. stacking limits)
end_of_game                            # victory determination
```

If a case needs an anchor that is missing, add it here in the same commit and mention it in the
commit message.

## Completeness check

Each book's registry must contain every case number found in its source text. Keep the counting
script in `tools/rules/` and report the result in your docket item: cases in source, cases in
registry, missing, extra.
