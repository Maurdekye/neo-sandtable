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
Record owner-private cargo lots with their goods totals, used CP and first carrier's allowance. At pickup, the lot's used CP becomes the greater of its inherited usage and the receiving carrier's current usage. While goods remain on the same physical trucks, add only their actual further carrying and handling CP to the lot's own history. A truck with 4 CP that moves another 3 CP gives its continuous load 7 CP, even if its new parent body has used 8 CP. The parent body's history does not make that load 11 CP. The greater-of rule applies only when goods change carrier. Fresh loads on trucks with differing histories require an explicit physical packing witness; unresolved histories remain a gap, never a body-CP guess. A fresh load unloaded at zero CP still retains that first truck's printed allowance. Changing carrier retains the first allowance. The receiving carrier's own allowance also applies independently.

A convoy's ceiling is the minimum printed extended allowance of its truck types: 30 for medium/heavy and 40 for light. First-line cargo records the original trucks' printed basic allowance, 20 or 25. Cargo continuously aboard its original first-line trucks may overrun with its parent; ordinary unloading clears that continuity exception. Final unloading into a dump remains legal after an original first-line overrun, with handling charged normally. The dump retains the exhausted lot even when its used CP exceeds its first ceiling. Loading it onto another carrier or moving it further is refused that stage; next-stage expiry makes the stock fresh.

Automatic expenditure and losses consume tagged lots before fresh untagged stocks. Prefer the greatest used CP, then the lower ceiling, then stable lot identity. Ask for parcel choices during loading or unloading only when histories differ.

Lot identities use per-side serials. Sites are explicit unit, pool, dump, coastal-ship or broken-vehicle marker identities; no pool is inserted in a unit-keyed map. Queries ignore earlier OpStage histories. Coastal ships may load a lot even after its truck ceiling is exhausted: the restriction concerns truck relays. Sailing adds no truck CP. Ship unloading and broken-vehicle cargo retain the truck history, and a later truck pickup still applies the inherited usage and first ceiling. Breakdown partitions and recovery use the physical allocation validated by the Land procedure.

## Rationale
The greater-of rule preserves both the goods' earlier travel and the recipient's already-used capability. Spending the least useful remaining parcels first is deterministic and gives the same choice a player would make without requiring an extra seat turn.

## Affected behaviour and tests
Convoy and first-line CP previews, handling, automatic fuel and stock debits, cargo partitioning, stage expiry, JSON checkpoints and paired enemy views. Exact movement and division hooks are coordinated with the Land procedures.
