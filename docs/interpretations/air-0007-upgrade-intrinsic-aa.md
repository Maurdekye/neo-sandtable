# air-0007 — Intrinsic AA while upgrading an air facility

- **Cases:** airlog:36.18, airlog:36.2, airlog:36.3, land:24.79
- **Status:** proposed
- **Profile version:** cna-2021-full (proposed; live AA integration pending)
- **Decided by:** rules-air-bases, 2026-10-07
- **Owner review:** pending

## Question

Does upgrading a landing strip or alighting area suspend its intrinsic anti-aircraft strength as well as its flight and maintenance functions?

## Evidence

The baseline in 24.79 makes a facility unavailable during an upgrade. It does not enumerate which functions are suspended. Case 36.18 assigns intrinsic AA against strafing and dive bombing; 36.2 gives strips the airfield functions, and 36.3 gives basins intrinsic AA. Neither section states that engineering work removes that defense.

Reading unavailability broadly would suspend every facility use, including intrinsic AA. Reading it as air operations would suspend takeoff, landing and maintenance while leaving the existing defensive strength intact.

## Ruling

Proposed: an otherwise surviving facility retains its intrinsic AA during an upgrade. Upgrade unavailability continues to block flight and maintenance. The intrinsic strength still applies only to strafing and dive bombing; this proposal grants no AA against other mission types.

The code currently returns Unsupported for a relevant intrinsic-AA query during an upgrade. It does not implement the proposed outcome pending a ruling.

## Rationale

The existing installation has not been destroyed when engineering work begins, and 36.18 supplies a specific defensive capability. Separating that defense from air operations avoids treating a construction restriction as physical removal without an explicit statement. The broader reading of 24.79 remains plausible, so this requires a recorded ruling.

## Affected behaviour and tests

The source helper in air::facilities holds this interaction as Unsupported. A regression asserts that a project-unavailable facility does not silently produce zero AA for a relevant mission, while unrelated mission types still receive zero intrinsic AA. After a ruling, tests must separately cover upgrading a strip and an alighting area, active and zero-capacity sites, and qualifying versus other attacks before live AA integration.
