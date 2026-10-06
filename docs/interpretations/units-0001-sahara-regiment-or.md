# units-0001 — Saharan detachment row: what the printed "or" covers

- **Cases:** land:19.32
- **Status:** adopted
- **Profile version:** v0
- **Decided by:** oob (proposed), 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
The Italian chart row for the Saharan detachment prints three infantry-battalion symbols, then the
word "or" and a cavalry-battalion symbol, then a machinegun battalion, a motorized machinegun
company and an artillery regiment. Does the "or" replace only the third infantry battalion, or
the whole group of three?

## Evidence
- The row reads: three infantry battalions, "or", one cavalry battalion, and then three further
  symbols that follow without any separator.
- Every other Italian row with an alternative uses "or" for a single slot (Bersaglieri slots in the
  semi-motorized division and in the Bersaglieri regiment).
- The oa sheet for the Saharan detachment (`it.sahara_det`) holds two Saharan Libyan battalions,
  a machinegun battalion, an MTR machinegun company and an artillery battery, so the formation was
  not three plain infantry battalions in the game either.

## Ruling
Read the "or" as covering the third infantry slot only: slots one and two are infantry battalions,
slot three is an infantry or a cavalry battalion, followed by the machinegun battalion, the
motorized machinegun company and the artillery regiment slot. Encoded as an `any_of` member in
`it.sahara_regt`.

## Rationale
This keeps "or" local, as everywhere else on the chart, and matches the sheet's actual contents.

## Affected behaviour and tests
`data/units/formations/it.toml` (`it.sahara_regt`); the assignment-capacity check for the
Saharan detachment (`land:19.2`).
