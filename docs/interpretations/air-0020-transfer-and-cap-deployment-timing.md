# air-0020 - Transfers share mission deployment and CAP timing

- **Cases:** airlog:33.0, airlog:37.13, airlog:40.27, airlog:42.12, airlog:42.15
- **Status:** proposed
- **Profile version:** cna-2021-full; operational caller not yet implemented
- **Decided by:** @neo-sandtable, 2026-10-20; relayed by @rules-mgr
- **Lead decision:** adopted for implementation; owner batch pending
- **Owner review:** pending

## Question

Does processing transfers first exempt them from CAP assigned in the same Operations Stage?

## Evidence

42.12 recommends handling transfers early. The Air sequence and 37.13 place mission counters during deployment, while 40.27 places route inspection after deployment. A transfer remains a tactical mission under 42.15.

## Ruling

Declare transfers during Mission Assignment. During Mission Deployment, place their destination counters together with all other mission counters, including CAP. Route inspection and interception follow that complete deployment. Transfers remain exposed to the current stage's CAP.

The early-transfer instruction is a bookkeeping recommendation. A later segment may process transfers ahead of its other missions, provided that ordering changes no outcome; it cannot bypass deployment, interception or combat.

## Rationale

This preserves both the transfer reminder and the explicit interception boundary instead of giving transfers an unstated escape from CAP.

## Affected behaviour and tests

No operational producer is implemented by this file. Future actual-flow controls must deploy a transfer and same-stage CAP through both owners' decisions before querying the transfer route. Reversing response order or privately partitioning an equivalent assignment must not change interception eligibility or RNG. Full guards remain until the real launch, combat and completion callers work. The public main SHA is reported for the owner batch only after authorized landing.
