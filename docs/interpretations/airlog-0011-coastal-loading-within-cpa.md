# airlog-0011: Coastal loading and unloading share the 50 CP ceiling

- **Cases:** airlog:56.31, airlog:56.34, airlog:56.35
- **Status:** adopted
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** rules-airlog, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2)

## Question
Does the coastal ship's 50 CP allowance exclude the loading and unloading costs?

## Evidence
Case 56.31 explicitly limits its allowance to movement. Case 56.34 separately charges 5 CP to load and another 5 CP to unload. Case 56.35 permits continuing between ports only within the ship's allowance.

## Ruling
Track one 50 CP budget per ship per OpStage. Loading, sailing and unloading all debit it. Loading at the beginning of the phase spends 5 CP; each sea hex spends 1 CP; unloading spends 5 CP. No fuel is needed.

## Rationale
The proposed shared ceiling treats the handling costs as part of the complete voyage budget. An alternative follows the movement-only qualification literally and allows 50 sailing CP plus handling. Owner review is pending; the lead authorized this proposed shared-budget implementation.

## Affected behaviour and tests
logistics::coastal validates cloned orders before mutation. A ship can load, sail 40 sea hexes and unload in one stage. A 41-hex path after loading leaves insufficient CP to unload. Repeated decisions and checkpoint recovery preserve this expenditure.
