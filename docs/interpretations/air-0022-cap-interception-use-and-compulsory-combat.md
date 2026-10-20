# air-0022 - Interception use and later compulsory CAP combat

- **Cases:** airlog:40.23, airlog:40.26, airlog:40.27, airlog:45.11, airlog:45.13, airlog:45.14
- **Status:** proposed
- **Profile version:** cna-2021-full; operational CAP ledger and combat caller pending
- **Decided by:** @neo-sandtable, 2026-10-20; relayed by @rules-mgr
- **Lead decision:** adopted for implementation; owner batch pending
- **Owner review:** pending

## Question

How does optional interception consume fighter availability, relocate its survivors and interact with compulsory target combat?

## Evidence

40.23 gives offensive CAP adjacent reach at a threshold of three assigned fighters. 40.27 limits interception use and permits a flight to face several encounters at different hexes. 40.26 and 45.14 require later offensive-CAP combat, with the neighboring-hex choice described by 45.11.

## Ruling

Count optional 40.27 interception use by actual fighter identity for the current Operations Stage. A fighter may join one such interception. The CAP owner may commit any subset, assigning each fighter to one flight.

Optional interception may occur at the CAP hex. It may also occur in an adjacent hex when at least three fighters were assigned to offensive CAP at that CAP hex. Preserve that assignment threshold; do not replace it with an unspent-fighter threshold.

Committed interceptors move to the encounter hex. For each intercepted flight, resolve encounters in route order before target-hex air combat; the flight's survivors continue. Interceptor survivors return to their CAP hex and retain offensive CAP for the phase. Their prior interception does not exempt them from compulsory combat.

During target-hex combat, all surviving offensive-CAP fighters at a CAP hex attack enemies in that same hex first. If there is no enemy there and at least three CAP fighters survive, the owner chooses exactly one neighboring enemy mission hex; all those surviving fighters move and attack together. Below three survivors there is no neighboring-hex obligation or reach. This later threshold counts survivors, unlike the earlier optional-interception assignment threshold.

## Rationale

The reading prevents record splitting from granting extra interception use and prevents voluntary interception from evading compulsory combat. It distinguishes the two source thresholds and keeps successive encounters in their proper flight order.

## Affected behaviour and tests

No ledger or combat dispatch is added by this file. Future controls cover subsets, once-per-fighter use, repeated attempts through another counter for the same flight, route-ordered encounters in distinct hexes, return to CAP and compulsory own-hex priority. Paired threshold cases distinguish three assigned fighters from fewer unspent fighters, and three survivors from two. Compulsory neighbor choice is a player decision, not an arbitrary identifier order. Persistent plane IDs remain private. The main SHA is reported only after authorized landing.
