# land-0045 — Disclose combat procedure facts at resolution

- **Cases:** land:3.6, land:3.62, land:10.34, land:10.35, land:12.46, land:14.23, land:15.22, land:15.51, land:15.54, land:15.56, land:15.73, land:15.94, land:16.46
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable, 2026-10-14
- **Owner review:** pending
- **Lead decision:** md9ed9f017398, recorded in the combat docket; Rules919/920 relay

## Question

Which combat aggregates and results can opponents exchange, and at which point, without
exposing the private compositions that produced them?

## Evidence

Land:3.6/3.62 permits the facts required by tabletop resolution while protecting other enemy
details. Holding-off, AA and assault each have a specific disclosure point; probes are revealed
after resolution. The lead read these source procedures when deciding the following boundary.

## Ruling

After all barrages resolve, disclose one sufficiency boolean for each holding-off barrage to
both sides, before retreat-before-assault and force assignment. Reveal neither the enemy
battalion-equivalent count nor the numerical threshold behind that boolean.

For each AA exchange, after both sides' rolls, disclose strengths fired, dice and damage,
plus the 3.62 armor facts: participating tank/SP types and recce/halftrack presence, or that
none participated. Exclude quantities, origins and the points chosen to absorb damage.
The reassignment option may convey only the eligibility information supplied by 14.23/16.46
at that time. Dummy removal is private to its owner. Earlier reads, validations and errors must
not disclose armor; force assignment cannot reject because of undisclosed target armor.

For each close assault disclose participating unit types, both Actual CA strengths, the basic
differential, shifts by category, the final differential, both CRT rolls/results and surrender.
Shift categories include terrain/edge, net morale, organization, the raw-strength ratio and
the 15.54/15.56 special adjustments. Results include loss percentage, Engaged, required retreat,
and captured percentage with its capture die. Probe/full status is revealed only afterward.

Do not disclose raw strength totals, individual adjusted morale, cohesion, morale dice, CP,
composition, actual TOE/raw casualties or casualty absorbers. A cost inferred from a public
differential is acceptable. The captor learns the prisoners/equipment actually transferred,
without additional private context. Public roll counts reveal only source-authorized facts,
including the barrage Truck roll when trucks exist.

## Rationale

The boundary follows the information exchanged to perform the tabletop procedure, rather than
publishing every computed intermediate. Raw totals and individual modifiers were rejected.

## Affected behaviour and tests

S1 adds no projection, event, decision or visibility code. Every later such change remains
STRICT and requires independent server-review through Platform; design approval is insufficient.
Future paired-state tests compare ordered observer streams/requests, validation, retries and
recovery, not snapshots after equal Advance counts. Owner review remains pending until a batch
after this file is published on main.
