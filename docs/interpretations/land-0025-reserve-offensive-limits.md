# land-0025 - Offensive limits of reserves

- **Cases:** land:18.22, land:18.23, land:18.24
- **Status:** proposed
- **Profile version:** cna-full-v1 / cna-dev-v1
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** pending

## Question
Does Reserve II earn the extra disorganization for each attack or once for the stage? May an unreleased reserve initiate combat?

## Evidence
land:18.24 attaches the extra DP to participation in voluntary combat during the Operations Stage, without a per-action frequency. land:18.23 and land:18.24 grant movement and combat after release; land:18.22 separately permits unreleased Reserve I a single-hex move and forbids Reserve II movement.

## Ruling
A released Reserve II unit earns one extra DP in an OpStage if it voluntarily conducts any close assault, probe, anti-armor fire or barrage, regardless of how many such actions it undertakes. Unreleased reserves cannot initiate those actions, but defend normally. Reserve I may still move one hex per Movement Segment at its ordinary terrain cost; Reserve II cannot move. Released Reserve I uses full CPA and released Reserve II uses half CPA, rounded down; neither may voluntarily exceed that effective CPA. Each released unit may make only one offensive close assault, including a probe, per OpStage.

## Rationale
The stated accounting period is the stage. Release supplies the permission for voluntary fighting; defense does not exercise that permission.

## Affected behaviour and tests
The reserve procedure retains release status and a stage-level offensive-action ledger. Tests cover designation, first and later release, single-hex movement, effective CPA, the one-assault limit, one extra DP across different attacks, rollback, stage reset and checkpoint recovery.
