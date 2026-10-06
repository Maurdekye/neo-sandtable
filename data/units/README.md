# Units, equipment and organization

Everything the engine needs to know about *what a unit or plane is*, as opposed to *where it
stands* (that is `data/scenarios/`). Owner: `oob`. Schema reviewed by the lead before bulk entry.
Sources: land `4.44`–`4.49` (characteristics charts, OA sheets), `19.3` (formation chart), `4.43`
and `20`/`34` (schedules); see the citation rules in `CONTRIBUTING.md` §2.

**Status: DRAFT v0 for lead review. Example records are real values read from the charts, but the
files they would live in are not yet bulk-populated.**

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
- **Stacking points** are stored per unit (`stacking_points`): the counters print the value
  (`land:9.22`, `land:3.33`) and `land:9.4` maps it to the organizational level (division 5, super
  brigade 3, brigade/regiment 2, battalion 1, company 0). Values here follow the echelon through that
  table and were checked against the printed counters of every headquarters in the Graziani set-up
  and a sample of the others; the only deviations from the plain table found are the two Libyan Tank
  Command regiment HQs (Aresca, Trivioli: super brigade, 3). `echelon` is one of `company`, `battalion`,
  `brigade`, `super_brigade`, `division`, `battle_group`. Shell values (`land:9.2`) are rule-driven and not stored.
- **Ids** are lowercase snake_case, stable forever: `<nation>.<sheet>.<unit>` for units
  (`it.1ccnn_div.129th_infantry_bn`: sheet id + slug of the printed unit name; a counter suffix is added only if two names in a sheet collide), `<nation>.<weapon>` for weapons (`it.m13_40`),
  `<nation>.<code>` for ID-code classes (`it.x`). Counter text (`201`, `7 RTR`) is **not** an id —
  it is not unique — and lives in `counter`.
- **Notes** are paraphrased; each OA footnote becomes a structured field where it has rules
  meaning (`reassign = { month = "1942-02", to = … }`), else a short `note`.

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
side = "axis"
gt = 9
opstage = 2
units = [ { unit = "it.xxv_corps_arty_regt", mode = "formation" } ]
trucks = {}
src = ["land:4.43b"]
```

One `[[arrival]]` / `[[withdrawal]]` / `[[replacement]]` row per printed schedule row; `att`/`less`
lists and weapon-type tags follow the printed legend; trucks arrive attached to units unless marked
`alone`. Designed for the whole war; only rows touching GT 1–6 are populated for Graziani's.

## Verification

Chart images are read directly (the retyped text of tables is OCR-scrambled in places); each file header names its source file names only (never contents). Each file carries `verification = "single" | "double"` in its header table; `double` means the chart
was read twice independently and diffed. Illegible or missing values are **omitted** and logged in
`GAPS.md`.
