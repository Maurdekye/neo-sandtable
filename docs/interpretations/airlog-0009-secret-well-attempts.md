# airlog-0009: Secret well attempts and repeated poisoning

- **Cases:** airlog:52.13, airlog:52.14, airlog:52.16, airlog:52.17, land:3.6
- **Status:** proposed
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** rules-airlog, 2026-10-07
- **Owner review:** pending

## Question
The poisoning retry restriction does not explain how it interacts with the opponent's secret attempt in the same Operations Stage. Well draws also need an immediate allocation before any uncarried result can become a stock.

## Evidence
Case52.16 makes a failed poisoning attempt end further attempts on that well for the stage;52.14 permits well conditions to remain secret until an opposing unit attempts to draw. Limited Intelligence3.6 keeps the rolls and quantities private. Case52.13 limits unlimited draws to what the unit can carry.

## Ruling
Each side may not retry its own failed poisoning attempt at the same well in the same stage. The opponent's private failure does not change the visible action menu. A draw spends its CP and records its result before the owner chooses immediate consumption, vehicle reserves, pasta water and legal truck cargo; unallocated water is discarded. No recorded draw becomes a generic supply source.

A unit without radiator water may spend the CP to operate its well. If the draw supplies its activity requirement, consume that reserve for the CP already spent. Otherwise it remains deprived of activity water and cannot move or offensively assault.

## Rationale
A global visible retry gate would disclose an otherwise secret enemy action. Private per-side retry history preserves the restriction for the acting player. Immediate allocation separates the rolled well yield from transportable holdings and permits the player to choose after seeing the result. The deprivation effects52.51 prevent movement and offensive assault, while drawing the well itself remains possible.

## Affected behaviour and tests
Well decisions and dice are secret. Failed draws at an opponent's depleted or poisoned well reveal only the condition after charging CP. Packing and CPA validation precede rolls; rejected allocations retain the same recorded draw. Tests cover finite tables, unlimited cities, retry limits, damaged pipeline chains, checkpointed allocations and condition-only enemy views.
