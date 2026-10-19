# airlog-0026 - Physical supply for cross-site squadron gun loading

- **Cases:** airlog:38.42, airlog:38.43
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full pending complete maintenance caller activation
- **Decided by:** neo-sandtable (lead), 2026-10-19, mc4d915dab177; relayed by rules-mgr mc26e9ef9b466
- **Owner review:** pending
- **Related ruling:** adopted, owner-reviewed airlog-0024 remains the squadron/period price authority

## Question

Which physical stock supplies gun loading when aircraft of one assigned squadron receive new ammunition at different facilities during the same maintenance period?

## Evidence

38.42 places ammunition at the servicing facility and applies refueling restrictions to rearming. 38.43 sets the gun price by squadron. Adopted airlog-0024 applies that charge once per maintenance period containing new loading of exact aircraft identities, without permanent credits or fees for preserving an existing load. That ruling does not identify a physical payer for distributed loading.

## Ruling

The present finite actual-SGSU implementation supports same-site new loading. It reports cited Unsupported for cross-site compound new gun loading, including a new site reached after a current-period historical charge. The recorded charge binds its actual facility, period, assigned squadron, payer and loaded identities; movement or removal of an SGSU cannot rewrite the past site.

The lead has also settled the reading for future cross-site activation. Exactly one Ammunition Point per squadron per maintenance period is spent at the facility of the first new gun loading, and that facility must hold the ammunition. Later new loading at another facility in that period spends no additional point, but each later servicing facility must hold at least one Ammunition Point or that loading refuses. This also applies to historical first-site loading followed by a later site. No implicit ammunition transfer, extra debit or refund follows.

The first site and paid history follow the actual deterministic finish order, never response arrival order. If an actual AI game encounters the bounded cross-site stop, the next extension implements this lead-decided reading without another product round trip. The same-site slice does not silently activate it or alter airlog-0024's adopted price.

## Rationale

The reading preserves the adopted single squadron charge while requiring physical ammunition at every site performing service. The bounded implementation retains an explicit stop until that additional source-presence procedure works.

## Affected behaviour and tests

Same-site multi-SGSU gun loading shares one charge. Current and historical cross-site new loading stop without spending stock, changing preparation or advancing RNG. Charge-site history survives SGSU movement, deletion and checkpoint recovery. Future cross-site tests must cover first-site payment, later-site stock presence without a second charge, absent-stock refusal, deterministic ordering and private action/Advance streams. No mission or flight entitlement is created.
