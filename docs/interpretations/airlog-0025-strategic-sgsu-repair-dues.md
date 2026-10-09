# airlog-0025 - SGSU dues for strategic refit

- **Cases:** airlog:35.14, airlog:38.32, airlog:48.0
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full, pending reviewed maintenance caller activation
- **Decided by:** neo-sandtable (lead), 2026-10-09, Rules461 provisional implementation ruling
- **Owner review:** pending NEXT consequential owner batch
- **Implementation:** provisional reading authorized; no new dues payment or maintenance caller is implemented here

## Question

Which SGSU fuel/water dues period permits strategic refit when Strategic Maintenance follows the final Operations Stage and has no new OpStage number?

## Evidence

35.14 prices SGSU Stores per GameTurn and fuel/water per Operations Stage, and disallows repair without its required supplies.38.32 assigns strategic aircraft refit to Strategic Maintenance.48.0 places that phase after the three Operations Stages. That timing identifies the latest completed stage but does not itself authenticate a supply receipt.

## Ruling

Strategic SGSU refit requires the CURRENT GameTurn Stores receipt and the fuel/water receipt for the LAST COMPLETED Operations Stage of that GameTurn. In the existing three-stage sequence this is stage3. No fourth SGSU fuel/water dues payment is invented for Strategic Maintenance.

Use the exact actual SGSU and trusted paid-period receipts; a missing, old-GameTurn, wrong-stage or legacy-absent receipt creates no credit. Cursor position alone cannot mark dues paid. Aircraft servicing costs and source-supported no-SGSU/off-map exceptions remain separate; this ruling neither grants blanket free supplies nor expands the source sentence about repair into every flight/refueling permission.

## Rationale and alternatives

The lead selected the last completed stage's actual paid status plus current GameTurn Stores. Creating a fourth payment adds a cost absent from the source. Accepting any earlier receipt or waiving fuel/water simply because no OpStage is active fabricates eligibility. These alternatives are retained but not selected.

## Affected behaviour and tests

Pin strategic completion afterstage3 with exact current Stores and stage3 fuel/water; stale GameTurn/stage2/missing/legacy receipts do not qualify; no automatic fourth debit, cursor-derived credit or duplicate charge. Checkpoint/reentry preserves paid provenance, fixed both-Air windows remain public, and actual missing-stock/dues validation fails on the disposable finish without partial servicing or RNG commitment.
