# land-0041 — Partial infantry carriage in breakdown accounting

- **Cases:** land:8.92, land:8.95, land:21.43, land:21.45; airlog:54.2
- **Status:** provisional — decided by neo-sandtable 2026-10-09 (lead reading m84413195a551, rulings m6a98c6d494ae)
- **Profile version:** cna-full-v1 / cna-dev-v1; invariant correction in BOTH profiles, not an optional variant
- **Decided by:** neo-sandtable, 2026-10-09; lead reading m84413195a551 and exact rulings m6a98c6d494ae (Rules408 relay m4e4ff758acd7)
- **Owner review:** pending next owner batch; lead-decided provisional interpretation, not owner-adopted

## Question
How should a valid infantry body with transport for only part of its strength survive a truck breakdown, without treating its walkers as passengers missing from a broken truck?

## Evidence
land:8.95 allocates truck duties by TOE and allows unused carrying capacity. land:8.92 makes enough capacity a condition for motorization; land:21.43 likewise separates insufficient carriage from motorized status. land:21.45 permits actual riders on broken trucks to dismount only through the paid procedure. Lead m84413195a551 resolves the partial-carriage reading: the carried part rides and the remainder walks. The engine's current transport assignment records vehicle capacity, not a separate passenger ledger; this accounting derives its represented whole-TOE obligation from the assigned capacity and current body strength.

## Ruling implementing the accepted lead reading
Let S be active infantry TOE before this loss, H(T) be native half-TOE capacity of assigned truck POINTS, E=min(S,floor(H(old)/2)) carried whole TOE, and W=S-E pre-existing walking TOE. A partially carried unit remains non-motorized while E<S. Truck ownership alone never grants truck CPA. E and W are explicit local accounting quantities; no new State, marker or wire field is added.

Given transport partitions working/origin/destination, retain adopted land-0028 split accounting: U=max(0,E-sum(floor(H(partition)/2))) is newly unresolved whole carried TOE. It excludes W. Broken passengers P must be within each broken partition's whole capacity, P<=E-U, and remaining carried E-P-U must fit working transport. Active body strength after loss is W+(E-P-U)=S-P-U. Markers retain P; unresolved accounting retains U; W never becomes unresolved or disappears. Each source-unit total remains S=active+P+U. Integer capacity does not create fractional infantry strength.

For captured S6/medium transport2 and a nontransport light-point loss, E2/W4/P0/U0: active strength stays6, transport2 stays assigned, and the body uses its normal foot CPA. For old light transport2 carrying one whole TOE, separating the two half-capacity points preserves land-0028's one unresolved whole point. A body with additional walkers keeps those walkers active while only that carried point becomes unresolved.

No free dismount, pickup by another carrier, CP transaction, cross-unit action, counter creation, proportional-loss choice, origin policy, RNG, cargo/fuel/water/cohort rule, visibility or transaction boundary changes. This adds a partial-carriage clarification; it does not rewrite or supersede land-0028's adopted origin/passenger/half-split protections.

## Rationale
The all-strength requirement incorrectly made ordinary pre-existing walkers consume transport after a nontransport truck loss. Separating W from E applies the rule's motorization threshold without paying dismount CP for men who were already walking or sending broken-truck riders to the destination for free.

## Affected behaviour and tests
Owned baseline passenger selection, losses passenger validator and unresolved whole-point helper share E/W. Existing Land individual_allowance already gates truck CPA on sufficient total capacity; no Land change is needed. Nonignored tests cover partial carriage in an explicitly synthetic context, sufficient carriage, full/partial half-splits, source-unit conservation and malformed-passenger rejection without state mutation.

0041 is RESERVED by lead m6a98c6d494ae. The lead reports current main highest0040 and no other scratch0041 outside rules-airlog; that is lead-attributed inventory, not a fresh remote query here. Later Land proposals use0042+. This file joins the next owner batch. Owner adoption remains pending; the provisional status is unchanged by engine implementation.
