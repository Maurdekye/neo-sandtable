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

- **Nations / id prefixes.** `it` Italian, `ge` German, `cw` Commonwealth (all British, Indian,
  Australian, NZ, South African, Free French, Greek and Polish units share the `cw` namespace
  because they share one unit-characteristics chart (4.46a) and one tank/gun chart (4.47)).
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
- **Arrival codes** (`arrives`): `"D"` = deployed at start of the first scenario; otherwise
  `{ gt = N, opstage = M }`. The OA sheets print the pair as `a/b`; the order is **an open question**
  (`GAPS.md` U-001) and the raw printed string is kept in `arrives_raw` until it is settled. It does
  not affect Graziani's (GT 1–6) except for units whose OA entry is printed that way.
- **TOE of a unit instance** (`toe`): `"N"` normal (= class maximum), `{ under = n }` (U@n),
  `{ over = n }` (O), or an explicit weapons list `toe = [{ weapon = "cw.mk_vi_light", n = 10 }]`.
- **Stacking points** are *not* stored per unit: they come from `land:9.4` (table owned by
  `rules-land`) by `echelon` + `shell` status. `echelon` is one of `company`, `battalion`,
  `brigade`, `super_brigade`, `division`, `battle_group`.
- **Ids** are lowercase snake_case, stable forever: `<nation>.<sheet>.<unit>` for units
  (`it.1ccnn_div.219_lgn.129`), `<nation>.<weapon>` for weapons (`it.m13_40`),
  `<nation>.<code>` for ID-code classes (`it.x`). Counter text (`201`, `7 RTR`) is **not** an id —
  it is not unique — and lives in `counter`.
- **Notes** are paraphrased; each OA footnote becomes a structured field where it has rules
  meaning (`reassign = { month = "1942-02", to = … }`), else a short `note`.

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
name = "1st CCNN (\"23rd March\") Division"
basic_morale = 0
src = ["land:4.45"]

[[unit]]
id = "it.1ccnn_div.hq"
name = "1st CCNN Div HQ"
counter = "1 CCNN"
class = "it.g"
echelon = "division"
toe = "N"
arrives = "D"
parent = ""                    # "" = top of the sheet
src = ["land:4.45"]

[[unit]]
id = "it.1ccnn_div.219_lgn"
name = "219th Legion HQ"
counter = "219 Lgn"
class = "it.g"
echelon = "brigade"
toe = [ { weapon = "it.65_17_gun", n = 1 } ]
arrives = "D"
parent = "it.1ccnn_div.hq"
src = ["land:4.45"]

[[unit]]
id = "it.1ccnn_div.219_lgn.129"
name = "129th Infantry Bn"
counter = "129"
class = "it.x"
echelon = "battalion"
toe = "N"
arrives = "D"
parent = "it.1ccnn_div.219_lgn"
src = ["land:4.45"]

[[unit]]                       # footnote a: officially part of the Libyan Tank Command, begins attached to 1 CCNN
id = "it.1ccnn_div.i_m_tank"
name = "I(M) Tank Battalion"
counter = "I(M)"
class = ""                     # class code not printed on this row — see GAPS.md; engine takes it from sheet it.libyan_tank_command
arrives = "D"
parent = "it.libyan_tank_command"
begins_attached_to = "it.1ccnn_div.hq"
src = ["land:4.45"]
```

A Commonwealth armoured example (7th Armoured Division, sheet header `basic_morale = 2`):

```toml
[[unit]]
id = "cw.7_armd_div.4_armd_bde.6_rtr"
name = "6th Royal Tank Regt"
counter = "6 RTR"
class = "cw.g"                 # tank battalion-equivalent, max 10 tank points
echelon = "battalion"
toe = [ { weapon = "cw.mk_vi_light", n = 10 } ]
arrives = "D"
parent = "cw.7_armd_div.4_armd_bde"
src = ["land:4.45", "land:4.47"]
```

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
  missions = { f = "night", s = "day", r = "-", d = "-" }
  # per-mission value: "night" = N (may fly at night), "day" = ! (daytime only), "-" = not capable
```

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

Each file carries `verification = "single" | "double"` in its header table; `double` means the chart
was read twice independently and diffed. Illegible or missing values are **omitted** and logged in
`GAPS.md`.
