# air-0021 - Class contacts, distinct paths and intercepted flights

- **Cases:** airlog:34.73, airlog:34.74, airlog:37.13, airlog:39.16, airlog:39.17, airlog:40.27, airlog:45.22
- **Status:** proposed
- **Profile version:** cna-2021-full; contact projection and operational caller pending
- **Decided by:** @neo-sandtable, 2026-10-20; relayed by @rules-mgr
- **Lead decision:** adopted for implementation; owner batch pending
- **Owner review:** pending

## Question

What may a route query disclose when several private assignments share a deployed counter, and which planes does a commitment intercept?

## Evidence

34.73 groups deployed counters by fighter, bomber or transport class at a target hex. Counter changes do not report losses under 34.74. The route query in 40.27 precedes plane-count disclosure. Mission and squadron partitions may differ under 39.16 and 39.17. Exact aircraft models are disclosed for combat under 45.22.

## Ruling

Keep one public counter for each class present at a target hex. These counters remain until Return to Base; private casualties do not change them, even when no represented aircraft survives.

A query identifies an enemy counter. Only its requester receives the distinct flight paths represented by that counter. Each result is a sequence of hexes. It conveys no aircraft count, model beyond the public class, squadron, persistent identity or duplicate-record count.

For interception, a flight comprises every plane of one side sharing both destination hex and path, across all classes. A commitment identifies a counter, one returned path and an interception hex, then engages that entire flight. An escort following the bombers' path belongs to that same flight. Different destinations or paths distinguish flights; splitting an otherwise identical private record does not.

## Rationale

The public counter retains its source-defined meaning, while the interception unit preserves escort relationships and prevents private record partitioning from buying additional flights or revealing hidden multiplicity.

## Affected behaviour and tests

No new public projection or operational dispatch is implemented here. Planned pairs vary model, count, duplicate records and survivors behind the same class/hex counter. Public counters and earlier decision streams remain equal. Queried paths are deduplicated without leaking private partitions; interception includes the same-path escort even when selected through the bomber counter. Counters persist through total private losses and leave at the scheduled return boundary. Exact contact/view fields require pre-code agreement. The public main SHA follows authorized landing.
