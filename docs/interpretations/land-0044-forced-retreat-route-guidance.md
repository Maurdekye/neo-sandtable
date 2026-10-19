# land-0044 — Forced retreat direction is advice, movement restrictions are binding

- **Cases:** land:8.12, land:8.13, land:9.35, land:10.23, land:10.24, land:10.26, land:15.82
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable, 2026-10-14
- **Owner review:** pending
- **Lead decision:** md9ed9f017398, recorded in the combat docket; Rules919/920 relay

## Question

Must the engine rank retreat destinations by proximity to dumps/cities or by a least-resistance
metric, even though no such metric is defined and dumps may be private?

## Evidence

Land:15.82 distinguishes required displacement from suggested retreat direction. Movement
and ZOC cases supply concrete route restrictions, and 9.35 adds stacking and split groups.

## Ruling

The player selects each group's route. Advice to head toward nearby supply or a city does not
define a legality test. The engine must not reject a route for its direction or invent a
distance/cost ranking to enforce that advice. Requests may paraphrase it as guidance.

Binding requirements remain:

- Follow consecutive legal hexes. Do not enter enemy occupation. Enemy ZOC entry is barred
  unless a friendly combat unit supplies the specified exception. Movement directly between
  enemy-controlled hexes follows 10.24 and that same exception. Enforce terrain, CP and breakdown.
- Respect stacking throughout and at the endpoint. Distinct split groups cannot traverse the
  same hex.
- A group retreating k hexes finishes at least k hexes in hex distance from its start. Its
  endpoint must also be farther from every hex containing enemy participants in that assault
  than its initial position was.

The player may stop short of the required N hexes, voluntarily or through inability, taking
10% extra losses for each missing hex. Entry into a major city can cancel the remaining retreat.
Route validation must not become an oracle for undisclosed enemy facts.

## Rationale

The lead rejected a shortest-path, fuel, terrain or CP metric for an advisory sentence. This
does not relax the separately specified legal-movement or separation conditions.

## Affected behaviour and tests

No route procedure changes in S1. Later retreat proofs must exercise split paths, stacking,
friendly-combat ZOC exceptions, separation from every assault origin, shortfall choices and
major-city cancellation, including privacy and breakdown/recovery. Owner review is pending;
the file must be on main before batch submission.
