# land-0032 - Revealed dummy minefield removal boundary

- **Cases:** land:26.14, land:26.15, land:26.23, land:24.38, land:24.18, land:8.22
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-engineers, 2026-10-07
- **Owner review:** pending

## Question
Does a revealed dummy disappear immediately, after the current Movement Segment, or after the Movement Phase containing subsequent repeated movement-and-combat cycles in the same Operations Stage?

## Evidence
The construction summary in 24.38 ties dummy removal to revelation. Case 26.15 makes that revelation immediate on enemy entry. In contrast, 26.14 names the end of the Movement Segment containing enemy entry. Case 26.23 instead uses Movement Phase and keeps charging later entrants during that phase. The Demolition Chart's dummy row names the Movement Segment. Case 8.22 permits repeated movement-and-combat segments, so these word choices can change costs in a later cycle.

## Ruling
Enemy entry immediately discloses the counter's dummy identity. Retain the counter and its entry surcharge until all movement and its continuations in the current Movement Segment have finished. Then remove it. Subsequent cycles have no counter and no surcharge. Do not remove it during an individual unit's move or while another movement decision remains open.

## Rationale
The dedicated minefield procedure in 26.14 and the chart agree on Segment. Case 26.23 expressly retains the revealed counter so later entrants still pay, which immediate removal under the broader construction summary in 24.38 would prevent. Apply the specific 26.14 and 26.23 timing procedures over that summary. Reading Phase as the current movement window preserves those later-entry costs. Keeping the counter for every later cycle would extend its life beyond the explicit Segment boundary.

## Affected behaviour and tests
The engineering movement-boundary callback removes revealed dummies after the owning movement window closes. Planned tests cover two units entering in one window, immediate type disclosure without immediate removal, and a new repeated cycle without the surcharge. Pair tests compare real and dummy states before first enemy entry, including advertised choices and acceptance. These tests are planned; no procedure has landed yet.
