# airlog-0018 - Physical movement cohorts and retained fuel accounts

- **Cases:** airlog:49.12, airlog:49.13, airlog:49.16, land:8.56, land:21.25, land:21.29
- **Status:** adopted
- **Profile version:** cna-2021-dev; cna-2021-full
- **Decided by:** rules-airlog, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2)

## Question
A detachment, breakdown or casualty can change a moving unit's composition after fuel was charged. Recomputing earlier movement using the new composition can refund lost vehicles, erase previous CP, or charge a transferred truck's first chart bucket twice. Splitting a group can also duplicate its rounded source credit.

## Evidence
Fuel depends on moving vehicle strength and the printed movement chart (49.12-49.13). The source identifies the movement origin (49.16). Truck division is permitted during detachment (8.56), and breakdown groups with different histories are assessed separately (21.25,21.29). Neither passage specifies how fuel-chart rounding credit travels between reassigned groups.

## Ruling
Record body movement CP independently from truck movement CP. Truck groups have persistent physical identities; a partial split creates a new identity linked to its parent, preserving the selected trucks' previous CP. A whole transfer retains its identity. Price new movement as the difference between the chart at a cohort's previous CP and at its previous CP plus the new movement. Keep already-paid body charges when vehicle TOE is lost, and price only the surviving body's new movement. Never refund past movement.

The original moving group retains one funding account containing its origin, exact cumulative cost and per-source draws. A transferred truck's later increment uses that account and its remaining source credit; the receiving body's increment uses its own account. Credit is shared once across descendants, not copied to each one. Body and truck accounts cannot donate credit to each other merely because they meet. Current tanks may fund their own unit; external stocks remain constrained to the retained origin.

Broken truck records retain their physical cohort history for recovery. Removed or destroyed groups leave their earlier charges paid. A later movement segment begins new CP and fuel accounting at the current origin while physical identities remain available for independent OpStage breakdown history.

These are explicit bookkeeping conventions for the source's silence. An alternative is to reset the reassigned group's fuel accounting, which either loses paid credit or lets transfer erase past movement. Another is to recompute the entire body's past cost after losses, which incorrectly returns fuel.

## Affected behaviour and tests
The movement plan/spend interface retains cumulative per-unit moving CP. Shared-account snapshots keep hypothetical planning pure. Tests cover repeated splits, one-time credit, separate origins/body payments, exact cohort selection, removed and recovered trucks, vehicle casualties, new segments, legacy checkpoints, atomic failure and enemy-view secrecy.
