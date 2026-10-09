# airlog-0022 - Refit attempts and squadron Stores

- **Cases:** airlog:35.23, airlog:38.23, airlog:38.33, airlog:38.34, airlog:38.35, airlog:38.36, airlog:36.5
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full, pending reviewed maintenance caller activation
- **Decided by:** neo-sandtable (lead), 2026-10-09, Rules461 provisional implementation ruling
- **Owner review:** pending NEXT consequential owner batch
- **Implementation:** provisional reading authorized; this file implements the recorded decision, not a live maintenance procedure

## Question

Does the SGSU refit maximum limit attempted or successfully refitted aircraft, over which period, and how do foreign or nationality roll groups affect Stores pricing?

## Evidence

38.23 names an Operations Stage or Strategic Maintenance Phase limit for refueling.38.33 gives the servicing SGSU a ready-plus-reserve refit limit, without repeating that period or specifying failed-attempt retries.38.34 separates foreign aircraft rolls;38.35 applies aircraft-nationality modifiers.38.36 prices an attempted squadron refit regardless of success, rather than pricing each dice roll.35.23 supplies the actual bound squadron total.

## Ruling

Each PlaneId may undergo ONE refit attempt in one maintenance period: one Tactical Operations Stage or one Strategic Maintenance Phase. Attempted IDs count against the servicing SGSU's bound ready-plus-reserve capacity, including failures. Splitting plans or changing servicing SGSUs does not create another attempt.

Charge one Stores Point ONCE for the assigned squadron undergoing refit in that period, irrespective of success or nationality roll partitions. Empty attempts charge nothing. Do not merge distinct assigned squadrons to reduce pricing or charge extra Stores for modifier-homogeneous roll groups.

The first bounded slice returns Unsupported airlog:38.36 for an assigned squadron split across servicing SGSUs, with the applicable full-profile missing-procedure guard. This is a missing allocation implementation, not a prohibition in the source or permission to fly. A later split implementation must allocate the SINGLE assigned-squadron charge, not charge per servicer.

Genuine verified36.5 unlimited aircraft maintenance/repair supplies include refit Stores; SGSU operation dues remain a separate component. German43.21 and Malta44.16 fuel/ammunition exemptions do not alone waive Stores. Source-extension gaps remain explicit Unsupported; no invented consumer or source stock.

## Rationale and alternatives

The lead selected parity with the explicit refueling period and bounded attempted-plane accounting. Counting successes only or allowing failed-plane retries would alter throughput and success odds. Charging every nationality roll or every servicing SGSU would incorrectly turn a squadron price into a partition price. These alternatives are retained but not selected.

## Affected behaviour and tests

Private period/attempt/cohort receipts and one atomic maintenance finish must pin failures consuming throughput, exact duplicate/second-servicer/reentry rejection, one squadron Stores charge across nationality groups, split Unsupported with unchanged state, checkpoint preservation, bound total versus ready-only, zero attempts and36.5/component-specific exemption behavior. Respond only records plans; no RNG or stock commitment survives a failed whole-State finish. No shared-field/caller/native/publication authority follows from this document.
