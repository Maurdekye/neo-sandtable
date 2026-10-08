# Units, equipment and organization

Everything the engine needs to know about *what a unit or plane is*, as opposed to *where it
stands* (that is `data/scenarios/`). Owner: `oob`. Schema reviewed by the lead before bulk entry.
Sources: land `4.44`–`4.49` (characteristics charts, OA sheets), `19.3` (formation chart), `4.43`
and `20`/`34` (schedules); see the citation rules in `CONTRIBUTING.md` §2.

**Status: charts and schedules for Graziani and the Italian Campaign are populated; whole-war schedules remain partial.**

## Layout

```
data/units/
  README.md  GAPS.md
  weapons/<nation>.toml           # weapon systems: tanks, guns, anti-tank, anti-air   (4.47 / 4.48 / 4.49)
  classes/<nation>.toml           # ID-code unit classes: CPA, close assault, max TOE   (4.46a / b / c)
  aircraft/<nation>.toml          # aircraft types                                       (4.44a / b / c)
  oa/<nation>/<sheet_id>.toml     # one OA sheet per file: roster of one parent formation (4.45)
  formations/<nation>.toml        # formation organization limits                         (19.3, 19.5)
  schedules/land_<side>.toml      # land reinforcements, replacements, withdrawals        (4.43, 20.x)
  schedules/air_<side>.toml       # aircraft and pilot arrivals / withdrawals             (34.8, 4.44 sched.)
```

`<nation>` is `it`, `ge` or `cw` (see the id-prefix convention below). Truck types and their characteristics are **not** stored here: `airlog:54.2` is a table owned
by `rules-airlog` (`data/tables/airlog/54.2-…`); scenario and schedule records refer to trucks only as
`{ light, medium, heavy }` point counts.

## Conventions specific to this folder

- **Id prefixes vs. nationality.** The id prefix is the *chart family*: `it` Italian, `ge` German,
  `cw` Commonwealth/Allied (they share one unit-characteristics chart, `land:4.46a`, and one
  tank/gun chart, `land:4.47`). Nationality is a separate, explicit field: every OA `[sheet]` has
  `side` (`axis` | `commonwealth`) and `nationality` (`italian`, `german`, `british`, `australian`,
  `new_zealand`, `indian`, `south_african`, `free_french`, `greek`, `polish`, ...); a `[[unit]]` may
  override `nationality` (e.g. the Free French Motor Marine Company on a British sheet). Rules
  that depend on it: Commonwealth withdrawals (`land:20.8`-`20.9`), morale, national restrictions.
- **Absent means omitted.** Unknown or not-printed fields are *omitted*, never `""` or `0`. The
  engine loads them as `Option`. A unit with no parent simply has no `parent` key. A value the
  source lacks gets a `GAPS.md` entry.
- **Ratings.** An omitted rating field means the chart prints "–" (not applicable *or* zero).
  A printed `0` is stored as `0`. A rating printed in parentheses `(n)` is stored as `n` plus
  `<field>_paren = true` (usable only when no hex-mate has non-parenthesized ammo-backed values,
  `land:3.4`, `land:4.46`). Close assault is `ca_off` / `ca_def`.
- **BAR** (breakdown adjustment, `land:21`): `bar = { shift = 1, dir = "R" }`; `bar = { shift = 0 }`
  for none. `dir` is `"L"` or `"R"`.
- **`cpa`** is the printed capability point allowance. `cpa_plus = true` when printed `10+` (historically
  supplied with trucks to be fully motorized); `cpa_fixed = true` when starred `30*` (keeps that CPA
  whatever the assigned TOE points have). `cpa = 0` with `emplaced = true` for printed `0`;
  `cpa_sited = true` for `0+`.
- **Arrival codes** (`arrives`): `"D"` = deployed at the start of the first scenario; otherwise
  `{ gt = N, opstage = M }`. Unit rows on OA sheets and the chart legends print the pair as
  `opstage/gt` (e.g. `2/68` = OpStage 2 of GT 68; chart notes such as "1/39 Game-Turn" read the same
  way), while the reinforcement schedules list `GT` then `OpS`. The printed OA string is kept in
  `arrives_raw` when it is not `D`.
- **TOE of a unit instance** (`toe`): `"N"` normal (= class maximum), `{ under = n }` (U@n),
  `{ over = n }` (O), or an explicit weapons list `toe = [{ weapon = "cw.mk_vi_light", n = 10 }]`.
  Both `{ under = n }` and `{ over = n }` assign **n TOE points at arrival** (`land:4.45`).
  The labels describe how that strength compares with normal TOE; n is the arriving strength,
  never an amount to add to or subtract from the class maximum.
- **Stacking points** are stored per unit (`stacking_points`): the counters print the value
  (`land:9.22`, `land:3.33`) and `land:9.4` maps it to the organizational level (division 5, super
  brigade 3, brigade/regiment 2, battalion 1, company 0). Values here follow the echelon through that
  table and were checked against the printed counters of every headquarters in the Graziani set-up
  and a sample of the others; deviations from the plain table include the two Libyan Tank
  Command regiment HQs (Aresca, Trivioli: super brigade, 3) and the 18th Australian Brigade (3). `echelon` is one of `company`, `battalion`,
  `brigade`, `super_brigade`, `division`, `battle_group`. Shell values (`land:9.2`) are rule-driven and not stored.
- **Ids** are lowercase snake_case, stable forever: `<nation>.<sheet>.<unit>` for units
  (`it.1ccnn_div.129th_infantry_bn`: sheet id + slug of the printed unit name; a counter suffix is added only if two names in a sheet collide), `<nation>.<weapon>` for weapons (`it.m13_40`),
  `<nation>.<code>` for ID-code classes (`it.x`). Counter text (`201`, `7 RTR`) is **not** an id —
  it is not unique — and lives in `counter`.
- **Notes** are paraphrased; each OA footnote becomes a structured field where it has rules
  meaning (`reassign = { month = "1942-02", to = … }`), else a short `note`.

## Engineering identity (`land:23.11`–`23.15`, `land:24.61`)

An OA unit may carry an `engineering` table with `scope`, optional `role` and
`toe_requirement`, `evidence`, and `src`. Scopes are `general`, `railroad_only`,
`road_only`, `anti_mine_only`, or `none`; omission remains Unknown. Explicit `none`
requires a verified non-engineer identity and has neither role nor TOE gate. A positive
scope needs a sourced `company`, `battalion`, or `headquarters` procedure role. This role
does not replace the printed `echelon`: the NZ railroad construction units retain their
OA battalion echelon while `land:24.61` gives their construction procedure a company role.

Evidence lists the exact local source filenames in `transcribed_from` and records
`verification = "double"`; only names and data are shipped, never source art. The first
batch contains three verified identities: Benghazi VIII/II and the 10th and 13th NZ
railroad construction companies. The 7th Armoured HQ counter flag remains pending under
U-012/U-014 and has no typed record. Alternate NZ chart files are
two visual reads of the same chart, not independent corroboration. No class-wide
engineering or non-engineering inference is made.

The optional gate is `{ weapon = "cw.scorpion", min_points = 6 }` under
`anti_mine_only`, citing `land:23.15`. A procedure must test actual identified current
points; the gate does not establish a refit date, grant a transfer, or imply readiness.
No Scorpion-gated unit is entered until its identity mapping is verified. Eligibility,
construction costs, minefield protection and activity history remain rules-layer decisions;
this table adds no runtime permissions or generalized protection flag.

## Field meanings (rating fields)

| Field | Unit | Meaning | Defined in |
|---|---|---|---|
| `cpa` | capability points | Capability Point Allowance: movement/combat budget per OpStage. | `land:3.5`, `land:6` |
| `aa` | AA points | Anti-air rating of a TOE point. | `land:3.5`, `airlog:46` |
| `barrage` | barrage points | Indirect-fire strength of a gun TOE point. | `land:3.5`, `land:12` |
| `anti_armor` | anti-armor points | Strength vs. armored vehicles in anti-armor fire. | `land:3.5`, `land:14` |
| `vulnerability` | points | Susceptibility of a gun to loss/capture in a forward position. | `land:3.5`, `land:12.1` |
| `armor_prot` | points | Armor protection vs. anti-armor fire. | `land:3.5`, `land:14` |
| `ca_off`, `ca_def` | points | Offensive / defensive close-assault ratings. | `land:3.5`, `land:15` |
| `fuel_rate` | fuel points per 5 CP (or fraction) of movement | Fuel burned by one TOE point. | `land:3.5`, `airlog:49.13` |
| `bar` | column shifts | Breakdown adjustment: `shift` columns, `dir` L or R. | `land:21.12`-`21.14` |
| `max_toe` | TOE strength points | Max TOE points a counter of this class may hold; `max_toe_paren` if printed `(n)` (HQ-held weapon points fight with parenthesized values, `land:3.35`, `land:4.46`). | `land:4.46` |
| `max_toe_extra` | TOE strength points | The printed "+n" allowance (e.g. AA points on Italian artillery class `kk`). | `land:4.46` |
| `range_hexes`, `tacair`, `maneuver`, `bomb_capacity`, `fuel_points` | hexes / points | Aircraft ratings. | `airlog:34.11`-`34.17` |

## Weapon systems (`weapons/<nation>.toml`)

```toml
[[weapon]]
id = "it.m13_40"
nation = "it"
kind = "tank"                  # tank | gun | anti_tank | anti_air_light | anti_air_heavy | …
name = "M 13/40"
cpa = 20
aa = 1
anti_armor = 3
armor_prot = 3
ca_off = 3
ca_def = 3
fuel_rate = 2
bar = { shift = 1, dir = "R" }
src = ["land:4.48"]

[[weapon]]
id = "cw.mk_vi_light"
nation = "cw"
kind = "tank"
name = "Mark VI Light"
cpa = 35
aa = 1
anti_armor = 0
armor_prot = 1
ca_off = 2
ca_def = 2
fuel_rate = 1
bar = { shift = 0 }
src = ["land:4.47"]

[[weapon]]
id = "it.65_17_gun"
nation = "it"
kind = "gun"
name = "65/17 Gun"
cpa = 15
barrage = 5
anti_armor = 0
vulnerability = 3
ca_off = 1
ca_def = 1
fuel_rate = 1
src = ["land:4.48"]
```

## ID-code classes (`classes/<nation>.toml`)

A *class* is a row of the Unit Characteristics chart: what a counter with that ID code can be.
HQ rows and artillery/tank/AT/AA rows hold weapon points rather than fixed ratings, so they list
which weapon `kind`s they may hold (`assigns`) and the maximum.

```toml
[[class]]
id = "it.x"                    # Italian ID code "x": a standard infantry battalion
nation = "it"
code = "x"
unit_type = "infantry"         # headquarters | infantry | tank | recce | artillery | anti_tank | anti_air | engineer …
echelon = "battalion"
cpa = 10
ca_off = 1
ca_def = 1
max_toe = 5
src = ["land:4.46b"]

[[class]]
id = "it.kk"                   # Italian artillery battalion-equivalent that may hold some AA
nation = "it"
code = "kk"
unit_type = "artillery"
echelon = "battalion"
cpa = 15
assigns = [ { kind = "gun", max = 9 }, { kind = "anti_air", max = 3, note = "heavy or light AA, not both" } ]
max_toe = 9
max_toe_extra = 3              # the printed "+3"
src = ["land:4.46b"]

[[class]]
id = "it.g"                    # Italian HQ that may assign one artillery point (e.g. a Legion HQ)
nation = "it"
code = "g"
unit_type = "headquarters"
echelon = "brigade"            # echelon is set per OA unit when the class does not fix it
cpa = 20
assigns = [ { kind = "gun", max = 1 } ]
max_toe = 1
src = ["land:4.46b"]
```

## OA sheets (`oa/<nation>/<sheet_id>.toml`)

One file per printed OA sheet (`land:4.45`). The sheet header gives the basic morale; each unit row
gives name, counter text, ID code, TOE/weapons, arrival, and its place in the assigned hierarchy
(indentation → `parent`). **Assigned** parent (`parent`) is the OA hierarchy; *attached* status is
scenario/runtime state and is never stored here.

```toml
[sheet]
id = "it.1ccnn_div"
nation = "it"
side = "axis"
nationality = "italian"
name = "1st CCNN (\"23rd March\") Division"
basic_morale = 0
src = ["land:4.45"]

[[unit]]
id = "it.1ccnn_div.1st_ccnn_div_hq"
name = "1st CCNN Div HQ"
counter = "1 CCNN"
class = "it.g"
echelon = "division"
toe = "N"
arrives = "D"
# no `parent` key = top of the sheet
src = ["land:4.45"]

[[unit]]
id = "it.1ccnn_div.219th_legion_hq"
name = "219th Legion HQ"
counter = "219 Lgn"
class = "it.g"
echelon = "brigade"
toe = [ { weapon = "it.65_17_gun", n = 1 } ]
arrives = "D"
parent = "it.1ccnn_div.1st_ccnn_div_hq"
src = ["land:4.45"]

[[unit]]
id = "it.1ccnn_div.129th_infantry_bn"
name = "129th Infantry Bn"
counter = "129"
class = "it.x"
echelon = "battalion"
toe = "N"
arrives = "D"
parent = "it.1ccnn_div.219th_legion_hq"
src = ["land:4.45"]

# The I(M) Tank Battalion is ASSIGNED to the Libyan Tank Command, so its single canonical id
# lives on that sheet (it.libyan_tank_command.i_m). The 1 CCNN sheet prints it with a footnote
# ("begins attached to 1 CCNN"); that is recorded as a reference, never as a second unit:
[[mention]]
unit = "it.libyan_tank_command.i_m_tank_bn"
begins_attached_to = "it.1ccnn_div.1st_ccnn_div_hq"
src = ["land:4.45"]
```

A Commonwealth armoured example (7th Armoured Division, sheet header `basic_morale = 2`):

```toml
[[unit]]
id = "cw.7_armd_div.6th_royal_tank_regt"
name = "6th Royal Tank Regt"
counter = "6 RTR"
class = "cw.g"                 # tank battalion-equivalent, max 10 tank points
echelon = "battalion"
toe = [ { weapon = "cw.mk_vi_light", n = 10 } ]
arrives = "D"
parent = "cw.7_armd_div.4th_armored_bde_hq"
src = ["land:4.45", "land:4.47"]
```

### Id rules and the validator

One canonical id per unit, forever: a unit's id is `<nation>.<sheet>.<...>` of the sheet it is
**assigned** to. Other sheets that print the same unit use `[[mention]]`. `tools/units/validate.py`
(owned by `oob`) fails on duplicate unit ids, dangling `parent` / `class` / `weapon` / `unit`
references, mentions of unknown units, and unknown field names; it is run before every push.

Optional unit fields: `basic_morale` (per-unit morale on sheets that list it per row; otherwise the
sheet value applies), `group` (printed grouping label with no HQ counter), `engineer_hq` (the
printed superscript E: engineering-capable HQ, `land:23.14`), `immobile`, `never_arrived_parent`
(asterisked: assigned to a parent that never reached Africa, `land:19.27`), `kind = "fixed_ship"`
(the San Giorgio), `stacking_points` and `echelon_symbol` (read from the counter, `land:9.22`: the
printed Stacking Point value, not the unit name, defines the organizational level).

## Formations (`formations/<nation>.toml`)

The Formation Organization charts (land section 19.3) say what a parent formation is *assigned* in
the historical organization: how many battalions, regiments and support units, and of which kinds.
Case numbers: **19.31 Commonwealth** (`cw.toml`), **19.32 Italian** (`it.toml`), **19.33 German**
(`ge.toml`). Every printed symbol is one slot; the number printed in front of it is that unit's
stacking-point value, and the small mark above it (`I`, `II`, `III`, `X`, `XX`) is its echelon.

Two record types per file. A **kind** is one legend entry (what may fill a slot); a **formation** is
one composition row (what a parent contains). The engine matches real counters to a slot through the
kind's `match` and `classes`.

```toml
[[kind]]
id = "it.inf_bn"                 # <nation>.<slug>, unique per file family
nation = "it"
name = "Infantry battalion"
echelon = "battalion"            # same vocabulary as classes/OA; a regiment is "brigade" (land:9.2)
symbol_echelon = "II"            # the printed mark: I | II | III | X | XX
sp = 1                           # printed stacking points (omit when the chart prints none)
match = { unit_type = "infantry", echelon = "battalion", tags_any = ["leg", "motorized", "parachute"] }
classes = ["it.bbb"]             # optional: exact ID-code classes (each resolves in classes/)
fill_by = [ { kind = "it.at_co", max = 3 } ]   # optional: smaller units that may stand in
any_of_kinds = ["it.at_bn", "it.at_co"]        # optional: a kind that is a union of others
src = ["land:19.32"]

[[formation]]
id = "it.semi_motorized_inf_div"
nation = "it"
name = "Semi-motorized infantry division"
kind = "it.inf_regt"             # optional: the unit kind this composition belongs to (regiment rows)
echelon = "division"
sp = 5                           # the parent's own stacking points
periods = [ { gt_from = 19, gt_to = 70 }, { gt_from = 92 } ]   # optional; inclusive; gt_to absent = open end
designation = "Sahara"           # optional: the label printed under the symbol (a specific named unit)
applies_to = ["it.sahara_det.saharan_detachment_hq"]           # optional: OA unit ids it describes
members = [
  { kind = "it.inf_regt", sp = 2 },                            # one slot
  { kind = "it.at_unit" },                                     # no number printed
  { kind = "it.tank_regt", formation = "it.tank_regt_3bn", sp = 2 },   # slot filled by a sub-formation row
  { any_of = [ { kind = "it.brs_mot_inf_bn", sp = 1 }, { kind = "it.motorcycle_recon_co", sp = 0 } ] },   # printed "or"
]
exceptions = [ { holder_sheet = "it.gruppo_maletti", add = { kind = "it.inf_regt", sp = 2 } } ]
limits = [ { what = "infantry_battalions", max = 6 } ]         # chart-text caps
src = ["land:19.32"]
```

Conventions:
- `match.tags_*` name unit-level properties that classes alone do not carry (`bersaglieri`,
  `motorized`, `motorcycle`, `machinegun`, `cavalry`, `armored_car`, `armored_recon`, `tank_destroyer`,
  `light`, `heavy`, ...). OA units do not carry `tags` yet (GAPS U-019); until they do, the
  slot test can use only `unit_type`, `echelon` and `classes`, and tags are a documented intent.
- A period variant is a separate record (`cw.armd_div_i` .. `iv`). The reorganization rule
  (`land:19.25`) chooses the record whose `periods` contain the current Game-Turn.
- `sp` on a member must equal the kind's `sp` (or the referenced formation's `sp`); the validator
  checks it, and that every `kind`, `formation`, `classes`, `applies_to` and sheet reference resolves.
- Unreadable or ambiguous chart values carry a `gap = "U-nnn"` / `glyph_note` pointer into `GAPS.md`.
- `19.5` Maximum Attachment is a table owned by `rules-land` (`data/tables/land/`); not stored here.

## Aircraft (`aircraft/<nation>.toml`)

Rows with several lines of characteristics ("or") are stored as `[[aircraft.mode]]` entries; the
owner picks a mode each time the plane is readied (`land:4.44`).

```toml
[[aircraft]]
id = "it.cr42"
nation = "it"
name = "C.R. 42 Falco"
role = "fighter"               # fighter (needs pilot) | bomber | transport | recon | flying_boat
manufacturer = "Fiat"
src = ["land:4.44b"]

  [[aircraft.mode]]
  range_hexes = 43
  tacair = 3                   # tacair_paren = true if printed (n): may not initiate air-to-air combat
  maneuver = 28
  fuel_points = 1
  missions = { f = "night", s = "day" }
  # keys: only the missions the plane CAN fly are present; value = "day" (printed "!": not at
  # night) or "night" (printed "N": may also fly night missions). A "-"/"." in the chart = key omitted.
```

Mission letter codes (`airlog:34.18`, `airlog:39`). Fighters chart: `f` offensive or defensive CAP
and strafing, `s` scramble, `r` reconnaissance, `d` strafe and/or any bombing mission. Bomber/transport
chart: `d` (as above), `r` reconnaissance, `b` bombing (naval convoy and land support). The Blenheim
IVF's scramble `0` (night scramble only) is `s = "night_only"`; the Hurricane IID row marked `A`
(may also strafe armor) is `f = "day"` plus `strafe_armor = true`. Only the values `"day"`,
`"night"`, `"night_only"` and (for `d` only) `"strafe_only"` exist. Transport capacity is stored as `transport = { toe_quarter_points, or_half_tons, paradrop_only }` (TOE points in quarters, tons in halves, so no fractions); ratings printed in parentheses get `<field>_paren = true`.

(Bomber rows add `bomb_capacity`, `torpedo_capacity`, `transport`, `missions = { d, r, b }`.)

## Schedules (`schedules/*.toml`)

```toml
[[arrival]]
gt = 9
opstage = 2
location = "tripoli"
units = [ { unit = "it.unassigned_guns.xxv_corps_artillery_regt" } ]
src = ["land:4.43b"]
```

Each printed schedule row becomes `[[arrival]]` / `[[withdrawal]]` / `[[replacement]]` events.
A truck allotment restricted to one formation uses a separate arrival event for that formation
(GT15 OpStage1: 2nd Armored Division), so other arrivals cannot take those trucks. `att`/`less`
lists and weapon-type tags follow the printed legend; trucks arrive attached to units unless marked
`alone`. Designed for the whole war; rows touching GT 1-20 are populated for both group-one scenarios.

## Verification

Chart images are read directly (the retyped text of tables is OCR-scrambled in places); each file header names its source file names only (never contents). Each file carries `verification = "single" | "double"` in its header table; `double` means the chart
was read twice independently and diffed. Illegible or missing values are **omitted** and logged in
`GAPS.md`.

## Schedule selection and withdrawal transport

`covers_gt = [1, 20]` is inclusive. `partial = true` means later campaign rows remain unentered.
Each land arrival has `gt`, `opstage`, `location`, `units` and `src`; optional `trucks` gives light,
medium and heavy points for that printed row. A unit selector with `subtree = true` includes assigned
units whose OA arrival equals the row's stage. `hq_only = true` selects the headquarters counter.
Mandatory withdrawals use the same selectors, but include assigned units already present; `less`
excludes named subtrees. `transport = { truck_value_points, motorization_points }` preserves the schedule's Tpt pair.
The first number is the minimum Truck Value Points accompanying a withdrawal under full Logistics;
the second is the minimum Motorization Points under abstract Logistics. Neither is a physical
truck count. The Commonwealth schedule header stores the chart-footnote conversion as
`truck_value_halves = { light = 1, medium = 2, heavy = 4 }`, so a light truck point is half a value
point, a medium point is one, and a heavy point is two. Compare integer half-values to twice the
full-Logistics requirement. The abstract requirement uses the second number instead. No selection
of individual truck types is invented in these records. Source: land:4.43a chart footnote.

Air monthly rows retain the whole `gt_from`/`gt_to` interval even when the scenario stops mid-month.
`distribution = "even_per_game_turn"` constrains the weekly total; players choose its plane types
(airlog:34.84). Single `gt` rows are that turn's totals. Squadron withdrawal selectors have `role`,
`count`, `min_planes`; `min_bomb_points_each` is the required capability of each qualifying bomber.
Pilot/SGSU arrivals are player/rule driven (airlog:34.82 and airlog:34.83), not fixed chart schedule rows.

`maneuver_night` overrides an aircraft mode's daytime `maneuver` for night missions.
Rommel is a classless `kind = "commander"` unit with `commander = true`, `cpa` and `vehicle`.
OA class codes left blank by the source remain omitted (U-020); explicit weapon/TOE data still applies.
OA file headers record verification separately from the cited sheet and unit records.

## HQ movement fuel

Unit Characteristics charts (land:4.46a-c) contain no fuel column. A class's normal TOE count does
not identify the weapons or establish a consumption factor. Known weapon rates remain in the
weapon records, and airlog:49.13 separately gives the truck and reconnaissance rate. HQ TOE without
identified equipment must stay unresolved for movement fuel; see GAPS U-025 and proposed
interpretation units-0005. An omitted rate does not mean zero.


## Coastal ship counters

Files in ships/ hold the printed carrying capacities from airlog:56.31. File-level provenance
uses src, transcribed_from and verification. Each ships record has a unique id for an actual
counter, its printed designation, integer capacity_tons, defining src and source filenames.
An unreadable capacity is omitted and recorded in GAPS; it is never zero. Movement allowances,
loading costs and current cargo are procedural/state data and do not belong in this roster.
A scenario fleet's axis_coastal_shipping.roster names the file relative to data/units.


## Infantry ammunition identity

`infantry_kind` belongs to an OA unit, with the values `ordinary`, `machine_gun`, or
`heavy_weapons`. It describes the counter type used by `airlog:50.17` and `airlog:50.2`;
those procedures still decide the rate and applicability. Ordinary includes foot, motorized,
mechanized, motorcycle, marine, commando and airborne infantry counters when their symbols
show neither the machine-gun nor heavy-weapons type. This field never comes from a rating,
name substring or class-wide default. An engineer may share an infantry class code, so an
absent kind must remain absent.

Each classified row also carries `infantry_kind_evidence`, with the exact local OA and
counter filenames in `transcribed_from`, and `verification = "double"`. No source art is
included. The 2021 rules retype omits the component legend in section 4; its symbols were
checked against the original `orig79:land:4.22` legend on pages 7–8. There is no inferred
change to the 2021 combat rules.

The combined Graziani/Italian Campaign rosters contain 181 rows whose characteristics class
is infantry: 179 are classified (156 ordinary, 19 machine-gun, 4 heavy-weapons), and two are
explicit gaps. See U-026 and U-027. `tools/units/coverage.py` reports the counts for each
scenario and rejects an additional unlogged omission. Later arrivals outside GT1–20 are not
covered by this audit.


Scenario plane setup rows may give `composition_exception_with` as aircraft ids. The paired
Free French MS406/Potez63/11 rows use symmetric references for their scenario-specific mixed
squadron permission (`scen:60.42`, `airlog:35.21`). Squadron composition otherwise follows
the printed class restrictions. Refitted reserves remain part of the ready-plane count; the
flight-ready limit does not cap all refitted planes stored at the squadron (`airlog:35.26`).

### Parent organization mappings

Each `[[parent]]` row in a formation chart file names one OA `unit` and exactly
one capacity source: `profiles = [formation ids]` for a chart-defined parent, or
`oa_slots = [OA unit ids]` for explicit slots on a singular OA structure.
The slots are source templates; they do not change when a child is eliminated.
An explicitly empty `oa_slots` list means zero assignment capacity.

`evidence = { transcribed_from = [OA source, chart source], verification = "double" }`
and `src` accompany the mapping. A separate `attachment_maximum` table, when
printed, contains `units`, its own `evidence`, and `src`. Assignment slots never
supply an attachment maximum. Every OA unit can have only one mapping.

Profiles use the referenced formation's inclusive `periods`; omitted periods
mean all positive Game-Turns. Overlapping profile periods, unknown references,
and mappings with both or neither capacity source fail loading. A gap between
periods stays a gap: the engine cannot choose a neighbouring profile. The
content loader tracks each consumed formation file for campaign pinning.
