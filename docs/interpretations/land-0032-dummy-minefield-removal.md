# land-0032 - Revealed dummy minefield removal boundary

- **Cases:** land:26.14, land:26.15, land:26.23, land:24.18, land:8.22
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-engineers, 2026-10-07
- **Owner review:** pending

## Question
Does a revealed dummy disappear after the current Movement Segment or remain through subsequent repeated movement-and-combat cycles in the same Operations Stage?

## Evidence
Case 26.14 names the end of the Movement Segment containing enemy entry. Case 26.23 instead uses Movement Phase and keeps charging later entrants during that phase. The Demolition Chart's dummy row names the Movement Segment. Case 8.22 permits repeated movement-and-combat segments, so these word choices can change costs in a later cycle.

## Ruling
Enemy entry immediately discloses the counter's dummy identity. Retain the counter and its entry surcharge until all movement and its continuations in the current Movement Segment have finished. Then remove it. Subsequent cycles have no counter and no surcharge. Do not remove it during an individual unit's move or while another movement decision remains open.

## Rationale
The specific removal case and the chart agree on Segment. Reading Phase as that same movement window preserves the rule's requirement that subsequent entrants in that window still pay. Keeping the counter for every later cycle would extend its life beyond the explicit Segment boundary.

## Affected behaviour and tests
The engineering movement-boundary callback removes revealed dummies after the owning movement window closes. Planned tests cover two units entering in one window, immediate type disclosure without immediate removal, and a new repeated cycle without the surcharge. Pair tests compare real and dummy states before first enemy entry, including advertised choices and acceptance. These tests are planned; no procedure has landed yet.
