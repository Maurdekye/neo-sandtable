# map-0005 - Libya-Egypt frontier and geometric country sets

- **Cases:** land:4.1, land:8.37, scen:60.31, scen:60.34, scen:60.44
- **Status:** adopted
- **Profile version:** 2021 map baseline; area schema version 1
- **Decided by:** neo-sandtable, 2026-10-08
- **Owner review:** reviewed 2026-10-08 by the owner (batch review 4)

## Question

The dense blue ticked line differs from the key's Border example. Which line
defines the national frontier, and how can the country sets end at the coast
without inventing a partition through coastal land?

## Evidence

The lead adopts the blue line along section-C sides as national Border. Libya
and Egypt labels, Capuzzo/Sollum and the Omar place pair support orientation.
The gray crossed transport through cell centres is separate. Direct native
review records 83 complete sides and C4122/C4221 printed to the shoreline.
The lead independently confirms that coastal incidence: C4221 Libya and
C4122 Egypt, with water on the remainder. A later fresh source supplement
corrects one country label at the returning C3819/C3920 bend; incidence is
unchanged. The original 1979 map is absent from supplied sources, so an
original-map comparison and equivalence claim are unavailable.

## Ruling

Use all 84 printed incidences as adjacency walls, retaining full versus
printed-to-shore extent metadata. Fill every C cell except verified Sea;
unknown terrain participates as ordinary membership. If west and east connect,
or any component remains unassigned, refuse generation rather than add a wall.
Libya is A/B plus C-west; Egypt is D/E plus C-east. Known Sea belongs to neither
across the whole grid. Canonical seam aliases count in section intersections.
The restricted selectors are C intersect Libya and C/D intersect Egypt.

The existing requires_land consumer filter applies surveyed terrain separately.
Development omits Unknown; full rules refuse an incomplete land domain before
exclusions. Sea is never offered. No whole-map land classification is inferred.
Country tracing leaves historic movement-layer records unchanged; it does not
silently replace old border masks or rail-family abstentions.

## Rationale

The printed coastline terminus already closes the terrestrial separation when
known Sea is outside the fill. No seaward device or invented land cut is needed.
Geometric membership and terrain availability remain separate reproducible facts.

## Affected behaviour and tests

national-frontier.toml, frontier.py and generate_areas.py supply the country
selectors. Tests reject broken cuts, unknown bypasses, orientation disagreement,
uncited and duplicate incidences and unverified Sea exclusions; section aliases
and exact country intersections are covered. The compatible setup consumer
checks known land/coast, Sea, Unknown, full/development profiles and empty domains.
Source error rate is unmeasured; other-section inspection was reduced overview,
not an exhaustive native-side absence survey.
