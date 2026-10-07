# air-0007 — Intrinsic AA while upgrading an air facility

- **Cases:** airlog:36.18, airlog:36.2, airlog:36.3, airlog:36.4, land:24.79
- **Status:** adopted
- **Profile version:** cna-2021-full (provisional reading; live AA integration pending)
- **Decided by:** neo-sandtable (lead), 2026-10-07; relayed through rules-mgr
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 3)

## Question

Does upgrading a landing strip or alighting area suspend its intrinsic anti-aircraft strength as well as its flight and maintenance functions?

## Evidence

The baseline in 24.79 makes a facility unavailable during an upgrade. It does not enumerate exceptions for particular functions. Case 36.18 assigns intrinsic AA against strafing and dive bombing; 36.2 gives strips the airfield functions, and 36.3/36.4 give water facilities their corresponding functions.

## Ruling

Provisional lead ruling, 2026-10-07T08:36:56: upgrade unavailability suspends intrinsic AA. The helper returns zero intrinsic strength while the project makes the facility unavailable. Flight and maintenance remain unavailable as well. Once the upgrade finishes, the normal AA rule resumes for a surviving active facility: one intrinsic point against strafing or dive bombing, zero against other mission types. Zero-capacity facilities still provide no intrinsic AA.

The source helper implements this reading. The former Unsupported hold for this particular interaction is removed. This does not add a live engineering project, runtime facility import or flak integration by itself.

## Rationale

The lead applies the availability restriction in 24.79 to all facility uses, including its intrinsic defense. Section 36 supplies no exception for AA during engineering work. The separate limits on qualifying attack types continue to apply.

## Alternative considered

The original proposal would retain intrinsic AA during an upgrade because the installation survives while engineering works on it. That reading would distinguish the existing defensive structure from its unavailable air operations. The lead instead ruled that the unqualified availability restriction includes intrinsic AA; the surviving-structure argument remains an alternative rather than implemented behavior.

## Affected behaviour and tests

The helper air::facilities::FacilityState::intrinsic_aa follows operational availability. The regression upgrade_start_and_completion_suspend_and_restore_intrinsic_aa covers both a landing strip and an alighting area, active and zero capacity, qualifying and other attacks, the unavailable interval and its end, and checkpoint preservation of the unavailable flag. Existing capacity, damage, repair, compatibility and canonical catalog assertions remain unchanged. These are source-helper tests; live engineering and AA window integration remain separate work.
