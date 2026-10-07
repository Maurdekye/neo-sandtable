# airlog-0020 - Cargo CP history across carriers

- **Cases:** airlog:53.22, airlog:53.24, airlog:53.25
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full
- **Decided by:** rules-airlog, 2026-10-07, per lead ruling
- **Owner review:** pending batch 3; consequential

## Question
How does a carried load retain its used CP when it changes truck, and which parcels disappear when fungible stocks are consumed?

## Evidence
53.24 charges handling and requires unloading before another truck takes a load. 53.25 prevents a relay of fresh carriers from moving the same goods across the map in one stage. The first carrier's allowance limits the load. 53.22 distinguishes the first-line basic allowance from the convoy allowance and permits first-line trucks to overrun with their parent.

## Ruling
Record owner-private cargo lots with their goods totals, used CP and first carrier's allowance. At pickup, the lot's used CP becomes the greater of its inherited usage and the receiving carrier's current usage. Subsequent carrier CP, including handling, adds to that usage. Changing carrier retains the first allowance. The receiving carrier's own allowance also applies independently.

A convoy's ceiling is the minimum printed extended allowance of its truck types: 30 for medium/heavy and 40 for light. First-line cargo records the original trucks' printed basic allowance, 20 or 25. Cargo continuously aboard its original first-line trucks may overrun with its parent; ordinary unloading clears that continuity exception. Final unloading into a dump remains legal after an original first-line overrun, with handling charged normally. The dump retains the exhausted lot even when its used CP exceeds its first ceiling. Loading it onto another carrier or moving it further is refused that stage; next-stage expiry makes the stock fresh.

Automatic expenditure and losses consume tagged lots before fresh untagged stocks. Prefer the greatest used CP, then the lower ceiling, then stable lot identity. Ask for parcel choices during loading or unloading only when histories differ.

Lot identities use per-side serials. Sites are explicit unit, pool, dump or coastal-ship identities; no pool is inserted in a unit-keyed map. Queries ignore earlier OpStage histories. Unloading or coastal transport retains a truck load's history instead of refreshing it.

## Rationale
The greater-of rule preserves both the goods' earlier travel and the recipient's already-used capability. Spending the least useful remaining parcels first is deterministic and gives the same choice a player would make without requiring an extra seat turn.

## Affected behaviour and tests
Convoy and first-line CP previews, handling, automatic fuel and stock debits, cargo partitioning, stage expiry, JSON checkpoints and paired enemy views. Exact movement and division hooks are coordinated with the Land procedures.
