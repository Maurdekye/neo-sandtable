# map-0004 - Printed inland-water contact on a shared side

- **Cases:** land:10.21, land:8.37
- **Status:** adopted
- **Profile version:** planned lake-enabled map contract; current published schema unchanged
- **Decided by:** neo-sandtable, 2026-10-07, provisional direction
- **Owner review:** reviewed 2026-10-08 by the owner (batch review 4)

## Question

The source distinguishes lake effects from marine and river effects, but
provides no explicit lake glyph in the inspected terrain key. What amount
of printed inland-water contact should classify a shared side?

## Evidence

Land:10.21 treats lake as a separate ZOC feature. The native 2021 terrain
key inspected under land:8.37 has no explicit lake graphic. This is an
interpretation of the supplied map, not a threshold found in the rules
or a claim about original 1979 equivalence.

Source-only feasibility 0001 directly inspected twelve registered whole
sides and endpoints against the native map and terrain key:

- E3413/E3414, E3413/E3513, E3413/E3514, E3414/E3514.
- E3513/E3514, E3514/E3515, E3514/E3614, E3613/E3614.
- E2805/E2806, D3913/D3914, C4120/C4220, D3615/D3616.

The first eight are potential inland-water contact candidates, not eight
certified lake sides. Two have mostly terrestrial shared segments with
water near an endpoint. Others require midpoint, majority and readable
inland identity checks. The four remaining sides provide dry, marine and
transport controls. Before this ruling all twelve lake proposals were
Unknown and produced no masks. The immutable feasibility report hash is
`a009bd494acf1aa0188e21e6ffefe6fef0a39b131dd92c37123a94fd1fa0c98b`;
its exact locators, source pins and native images remain local only.

**Consequence count:** eight possible contact candidates have been found
in the twelve-side sample. This is a bounded count inside the corridor,
not an exhaustive count across its 2,541 physical pairs. That exhaustive
candidate inventory is still pending the current terrain/pilot cycle.
Report candidate, ambiguous and uninspected counts separately before
claiming a corridor total or presenting full owner consequences.

## Ruling

A lake hexside requires printed inland water to cover both the midpoint
and a majority of the shared side. Endpoint-only or minor contact does
not qualify. An unreadable or ambiguous side stays Unknown, with no
inferred absence. A water subtype stays unsourced unless printed.

Inspect the actual full shared side. Cell terrain, a port symbol, nearby
blue ink or a neighboring marine/river mask cannot substitute for that
observation. Preserve previous source records. A new lake layer never
backfills absence from older surveys or changes marine identity by alias.

## Rationale

Requiring both central contact and a majority gives the proposed side
feature a consistent geometric meaning without making a small shoreline
touch control the full side. The threshold is a provisional lead choice.
Separate Unknowns preserve uncertainty about unreadable extent and
inland-water identity for owner review or later reversal.

## Affected behaviour and tests

No lake records, masks, loader changes, costs or runtime behaviour are
implemented by this document. Land's consumer and map tooling must be
checked together in a later atomic publication, after current gap work.
The later tests must cover midpoint plus majority, minor and endpoint-only
contact, unreadable identity, missing coverage and independence from
marine/river masks. No unsupported lake cost is supplied here.

Pipeline is separately ruled to be Airlog/Engineering game state rather
than a required static map survey kind. Preserve its historical raw
records while removing their blocking weight only through the compatible
consumer/tooling change; do not reinterpret those records as absence.
