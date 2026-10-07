# air-0006 — Refit after transfer flight

- **Cases:** airlog:38.31, airlog:42.14, airlog:37.15, airlog:37.32
- **Status:** proposed
- **Profile version:** cna-2021-full (proposed; no operational implementation yet)
- **Decided by:** rules-air-bases, 2026-10-07
- **Owner review:** pending

## Question

Does a transfer flight leave a previously refitted plane refitted after it lands?

## Evidence

The 2021 baseline in 38.31 exempts transfers when describing flight that creates a new refit requirement. Case 42.14 instead explicitly requires refit following a completed transfer, while allowing the transfer itself to depart without prior refit. Cases 37.15 and 37.32 similarly allow transfer and emergency transfer to use fuelled aircraft without requiring prior refit.

These provisions agree on departure eligibility but disagree about the state of a previously refitted plane after landing. Neither method of refit in section 38 resolves that difference.

## Ruling

Proposed: a transfer may depart with either refit state, provided the plane satisfies its other flight conditions. On completion, mark the aircraft as needing refit. Apply the same rule to a successful emergency transfer, since 37.31 identifies emergency flight as a transfer.

An attempted emergency departure that fails its escape roll does not count as a completed transfer. It retains its pre-attempt refit state. Other consequences of failed departure remain governed by their own cases.

## Rationale

Case 42.14 addresses transfers specifically and distinguishes permission to depart from the maintenance requirement after arrival. Its explicit arrival requirement therefore controls the broader maintenance summary in 38.31. This preserves both the departure exception and the stated post-transfer maintenance cost.

## Affected behaviour and tests

Planned hooks: air transfer/emergency completion and maintenance eligibility. Tests will cover departure by a fuelled unrefitted aircraft, loss of refit after successful transfer, retained refit after a failed emergency departure, and inability to fly a subsequent non-transfer mission until refit succeeds. These tests and operational hooks are not implemented in this proposal commit.

## Provisional implementation authorization

The lead provisionally accepted this reading for implementation on 2026-10-07, relayed through rules-mgr and rules-air. The file remains proposed for the next interpretation review batch. This authorization covers the planned transfer hooks and tests; the proposal commit does not itself implement operational transfer behavior.
