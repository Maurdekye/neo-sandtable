# scen-0007 - Tobruk initial efficiency and the San Giorgio

- **Cases:** scen:60.7, airlog:55.12, airlog:55.18, airlog:55.25, airlog:55.3
- **Status:** adopted
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable via rules-mgr, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 3)

## Question

How does scenario60.7's Tobruk efficiency7 coexist with the bound chart maximum5 and the three-level ship blockage?

## Evidence

The local 2021 scenario60.7 assigns Tobruk7 while noting the partially sunk San Giorgio. The native55.3 chart image instead gives Tobruk maximum5 and describes a starting reduction because of the ship. Case55.12 independently uses5 as Tobruk's assigned level,55.25 gives the ship reduction as three levels, and55.18 forbids recovery above the assigned maximum. Structured scenario data accurately retains7; the chart transcription accurately retains5. The disagreement is in the sources.

## Ruling

Use the bound55.3 Tobruk maximum and subtract the three blockage levels specified in55.25 to obtain the initial current efficiency. Preserve those blockage levels so quiet-stage recovery does not remove the ship. Do not hard-code the resulting efficiency2. Keep the scenario's cited raw7 as an accurate transcription, and identify this interpretation at the consuming procedure. Apply starting conditions only to new canonical PortState; subsequent entry changes owner while existing efficiency, blocked/mined levels, damage-stage history and shared tonnage budget remain intact.

## Alternatives and rationale

Giving the scenario7 precedence would require an above-maximum port, or assuming maximum10 because seven plus three equals ten. Neither is supported by the verified chart, its own footnote, the55.12 example or the55.18 maximum rule. The lead chooses the mutually consistent chart maximum and specific ship blockage; this is an affirmative initial-efficiency ruling, not an Unsupported stop or a silent clamp.

## Affected behaviour and tests

Engineering owns the typed setup consumer and Airlog reviews canonical ports initialization and any error/callsite contract. Planned tests verify Graziani and inherited Italian initial efficiency from bound maximum minus source blockage, retention of the raw7, exact canonical identity matching, repeated initialization preserving damage/budget fields, accepted ownership changes only, and no invented maximum or capacity fraction. The pure source consumer and immutable loader validation cover the source reading; Airlog owns runtime initialization, unknown-policy refusals and preservation of existing port state.

## Public content validation

The supported exception requires exactly the authored Tobruk record at canonical C4807, citation scen:60.7 and raw efficiency7. Changed known values, out-of-range values, invalid anchors and contradictory available port identities are malformed public content. Missing icon evidence remains unknown and does not prevent loading or create a port. Structurally valid future authored policies remain diagnostics: loading succeeds, FULL port-procedure entry returns Unsupported uniformly before inventory branches, and DEV refuses efficiency-dependent operations at the affected port with a private source-labelled note. DEV never supplies an invented numeric efficiency; other ports remain usable.
