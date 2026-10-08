# airlog-0021 - One-stage pool activity reserve

- **Cases:** airlog:52.42, airlog:52.43, land:29.34, land:29.35
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full, pending reviewed caller activation
- **Decided by:** neo-sandtable (lead), 2026-10-08, selected reserve ruling
- **Owner review:** pending (consequential, batch 4)

## Question
How much water may a real second- or third-line truck pool receive as activity reserve, distinct from its carried water cargo?

## Evidence
52.42 requires water when vehicles or trucks use capability during an OpStage;29.35 doubles that requirement in global hot weather.29.34 distinguishes consumed water from carried water exposed to loss. Existing unit issue permits activity reserve only up to unmet stage need. The truck transport chart governs carried cargo; it does not create an evaporation-exempt multi-stage activity store.

## Ruling
At the fixed Water Distribution stock-issue round, issue to an owned resolved pool only up to its unmet current OpStage demand. Demand is the checked sum of its Light, Medium and Heavy Truck Points, doubled for hot weather. Existing activity reserve reduces that need. Baseline issues exactly unmet need when authorized finite stocks can cover it.

The cap applies only to new issues: preserve an existing reserve at or above current demand across stage changes or count reductions, issue nothing until it falls below demand, and never discard it or convert it to cargo.

Activity reserve remains independent of aggregate cargo capacity. This issue has no packing field or packing check. Larger carried quantities remain water cargo and cannot be converted to activity reserve by this procedure. Stock issue costs no CPA, draws no well die and introduces no paid-water ledger. Owner answers validate on disposable drafts; stock and reserve effects occur only at the existing batch finish.

## Rationale
The lead selected parity with unit activity issue and the consumed-versus-carried distinction. A reserve sharing cargo capacity but holding multiple stages of water would introduce an evaporation-exempt pool store. That alternative is rejected. No well-extraction or cargo-to-reserve entitlement follows.

## Affected behaviour and tests
Additive private WaterAnswer pool allocations, real identity and source authorization, hot/cold unmet bounds, finite-source debit, old answer decoding, baseline selection, Respond purity, closure rollback and owner-private observations. Real Graziani setup and stock issue followed by convoy Move remain required before caller activation. No new State, TruckPool, ledger or action window is introduced by reserve issue.
