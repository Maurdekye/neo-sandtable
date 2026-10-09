# airlog-0024 - Squadron gun loading charge per maintenance period

- **Cases:** airlog:38.23, airlog:38.41, airlog:38.42, airlog:38.43, airlog:38.44, airlog:38.47
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full, pending reviewed maintenance caller activation
- **Decided by:** neo-sandtable (lead), 2026-10-09, Rules461 provisional implementation ruling
- **Owner review:** pending NEXT consequential owner batch
- **Implementation:** provisional reading authorized; paid preparation admission/consumption remains a separate reviewed contract

## Question

How does the one-point squadron gun price apply to exact planes loaded through multiple servicing plans or maintenance periods, without charging untouched ammunition or granting permanently free future loads?

## Evidence

38.42 gives rearming the limits and restrictions of refueling.38.43 prices gun ammunition by squadron.38.41 permits unarmed aircraft.38.44/.47 price bombs, mines and torpedoes separately per plane. These distinctions do not establish an annual or permanent squadron credit, a fee merely for keeping guns loaded, or inferred ammunition depletion.

## Ruling

Charge ONE Ammunition Point per assigned squadron per maintenance period for NEW gun loading onto explicitly selected live PlaneIds. Exact IDs count against the servicing SGSU's rearming throughput; sharing a squadron charge never bypasses that limit. A period with no actual loading/change charges nothing. Preserve leaves the existing source-backed gun state untouched and pays zero, including after a period boundary.

Record the current-period squadron charge and exact loaded IDs atomically with the paid preparation effect. A past-period charge cannot authorize free new loading in a later period. No implied gun depletion or forced periodic reload follows. Aircraft without known gun state do not gain loaded guns from coarse flags or an old receipt; actual paid load admission and parent-owned consumption require their reviewed APIs.

The gun point remains separate from per-plane ordnance charges and source fuel. Genuine source-supported component exemptions remain distinct; none creates a permanent future-load credit.

## Rationale and alternatives

The lead selected one squadron charge per actual maintenance period containing loading. Charging each plane contradicts the squadron price; a permanent paid flag makes later loads free; automatic periodic fees charge untouched Preserve. These alternatives are retained but not selected.

## Affected behaviour and tests

Pin multiple exact-ID loads sharing one squadron charge within a period, distinct squadrons, rearming-throughput bounds, later-period new load charged again, cross-period Preserve zero, no inferred depletion and gun versus ordnance charge separation. Source gaps, paid-load admission failure, duplicate IDs and late debit failure roll back all receipts/load/stock/RNG. Own-private schemas, fixed batches and checkpoint/reentry tests remain required before live caller activation.
