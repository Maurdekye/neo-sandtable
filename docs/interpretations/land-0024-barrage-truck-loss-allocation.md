# 0024 — Barrage truck loss allocation and disclosure

- **Cases:** land:12.23, land:12.24, land:12.46, airlog:53.11, airlog:54.2
- **Status:** adopted
- **Profile version:** cna-2021-dev and cna-2021-full
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** pending (consequential; batch 2)

## Question

How does even loss distribution apply when the engine stores supply cargo in aggregate, and can a barrage's truck die reveal that trucks were present?

## Evidence

Land:12.46 gives the defender the loss choice while requiring distribution among truck and cargo types. It also adds the carriers of destroyed motorized infantry to the table losses. Land:12.23–24 limits the pre-plot target catalog to anonymous broad classes. The conditional truck roll in 12.46 subsequently reveals truck presence for an actually barraged hex. The supply and truck capacity charts use different point units for different commodities; equal commodity point counts do not mean equal loads.

## Ruling

The defender supplies an explicit packing of lost and surviving trucks and cargo. Both must fit their chart capacities and conserve actual holdings. Per-present truck-type removals differ by at most one truck point, except when a type is exhausted. Cargo losses use exact truck-equivalent capacity shares. Empty capacity and troop transport are categories alongside the four supply types. Per-present category losses differ by at most one truck-equivalent, except when that category is exhausted. The baseline allocates deterministically in even round-robin order, truck type then category.

All trucks attached to the target unit or its parent in the target hex are eligible, including empty, cargo and troop carriers. First remove the carriers of the infantry TOE actually destroyed: the minimum whole number of carrying truck points whose actual assigned capacity covers those points, rounded up. The defender cannot choose zero when the destroyed points were riding. Then apply the truck table result to the remaining trucks, capped by what remains. A table loss of a troop carrier dismounts its passengers and reduces motorization; it creates no additional TOE loss.

Roll on the truck row only when trucks are present. At resolution the public stream reports that the roll occurred and its CRT result. Actual truck counts, cargo composition and capped loss allocations remain private to the defender and operator. No truck presence means no truck roll or event. Campaign RNG draw counts remain hidden from seat streams.

## Rationale

The text gives no exemption for troop carriers. Separating the extra infantry-carrier loss from the subsequent table loss prevents counting a destroyed truck twice. Capacity shares compare different commodities without inventing equivalent supply-point units. Including empty capacity prevents selecting empty vehicles preferentially to spare supplies. The conditional roll itself provides its limited disclosure after plots close.

## Affected behaviour and tests

The combat barrage procedure builds anonymous target catalogs, resolves target and conditional truck dice, then asks the defender to allocate losses. The logistics capacity helper validates truck/cargo packing and conservation atomically. Regressions must cover mixed truck types and cargo, empty trucks, troop transport, exhausted categories, rejected allocations, no-truck RNG counts and opponent redaction.
