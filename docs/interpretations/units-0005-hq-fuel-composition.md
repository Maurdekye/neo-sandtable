# Headquarters fuel when equipment is unspecified

- **Cases:** airlog:49.12, airlog:49.13, airlog:49.14, land:3.31, land:3.32, land:3.34, land:3.35, land:4.46a, land:4.46b, land:4.46c
- **Status:** proposed
- **Profile version:** draft
- **Decided by:** oob, 2026-10-07
- **Owner review:** pending

## Question
How can movement fuel be priced for an HQ with a normal, unparenthesized TOE count but no identified vehicles or weapons?

## Evidence
The Commonwealth, Italian and German Unit Characteristics chart images were checked in full, including their HQ blocks and legends. Each prints movement and combat characteristics plus TOE limits; none contains a fuel consumption column or an HQ fuel default. The Tank and Gun Characteristics charts provide rates for identified weapon systems instead.

Airlog:49.12 includes HQs with unparenthesized TOE among fuel users. Airlog:49.13 prices moving vehicle points using their own consumption factor and groups of five movement CP, rounding a partial group upward. It supplies a rate of one for trucks and reconnaissance/armored-car points, without extending that rate to all headquarters. Land:3.34 and land:3.35 describe gun and tank points belonging to HQs; land:4.46 describes the characteristics of their actual equipment. The slower component's CPA in land:3.32 is not a fuel consumption factor.

In the current OA data, six Commonwealth HQs of classes cw.e/cw.f and three Italian HQs of class it.g have toe = "N" without a weapon list. Normal TOE establishes the count, not the vehicle model. The Italian class permits artillery, without identifying a gun. The Commonwealth rows provide intrinsic ratings without an explicit equipment choice domain.

## Ruling
For an HQ whose current TOE explicitly identifies weapons, sum fuel using each weapon's recorded rate and current point count. Price separately recorded first-line trucks separately; an HQ counter must not add another copy of its children's fuel bill. Apply the stated reconnaissance/truck rate only to points positively identified as those types.

A scalar or normal HQ TOE with no identified equipment has an unresolved rate. Do not treat the missing rate as zero, set every HQ to one, derive a rate from CPA, or choose a plausible gun. Before fuel-dependent movement, resolve the equipment through a player choice only where the class and source rules supply a legal equipment domain. Where no such domain exists, movement remains explicitly Unsupported, citing airlog:49.12 and this interpretation, in both profiles. The development profile may skip unimplemented phases; it must not execute a movement that silently consumes no fuel.

## Rationale
The rules establish an obligation to consume fuel but do not provide a general HQ price. Keeping the missing identity visible preserves both facts and prevents unsupported movement from creating free transport.

## Affected behaviour and tests
GAPS U-025 tracks the missing rates; the class records gain no invented fuel field. This proposal is a contract for the movement fuel API, not an implemented fuel procedure. Rules-airlog owns its implementation and tests: explicit weapon mixtures and trucks use their known rates; bare normal HQ TOE returns Unsupported without changing fuel or position.
