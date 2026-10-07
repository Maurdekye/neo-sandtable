# Headquarters fuel when equipment is unspecified

- **Cases:** airlog:49.12, airlog:49.13, airlog:49.14, land:3.31, land:3.32, land:3.34, land:3.35, land:4.46a, land:4.46b, land:4.46c
- **Status:** adopted
- **Profile version:** cna-2021-dev / cna-2021-full
- **Decided by:** the owner, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2; house rule chosen)

## Question
How can movement fuel be priced for a headquarters with an unparenthesized TOE count but no identified vehicles or weapons?

## Evidence
The Commonwealth, Italian and German Unit Characteristics charts were checked in full, including their HQ blocks and legends. They give movement and combat characteristics and TOE limits, but no fuel consumption column or general HQ fuel rate. Tank and Gun Characteristics charts supply rates for identified equipment instead.

Airlog:49.12 includes headquarters with unparenthesized TOE among fuel users. Airlog:49.13 prices vehicle points with their consumption factor and movement CP grouped in fives, rounding a partial group upward. Its factor of one applies to truck and reconnaissance points; the text does not assign that factor to all headquarters. Land:3.34/3.35 distinguish an HQ's own gun or tank points from its attached units. CPA is not a consumption factor.

The current OA records contain six Commonwealth cw.e/cw.f headquarters and three Italian it.g headquarters with normal TOE but no weapon composition. Their scalar counts identify strength without establishing an equipment model.

## Ruling
This is a **HOUSE RULE chosen by the owner**, not a reading of the printed rules. A headquarters whose current TOE is a normal numeric count with no identified equipment consumes movement fuel at factor one for each of its own TOE points. Price these points through the same factor-one vehicle path as truck and reconnaissance points, including the existing Fuel Consumption Chart rounding in interp:airlog-0001. This house rule adds no separate HQ rounding rule. The owner chose the truck and reconnaissance factor so these headquarters can participate in movement rather than remain frozen by missing equipment data.

The canonical classifier is the source UnitClass.unit_type of headquarters. The scalar count must be unparenthesized (max_toe_paren is false), and the current TOE must be Normal, Under or Over rather than an explicit Weapons list. Normal resolves to the recorded maximum; Under and Over hold the actual current counts. The nine current affected records are the cw.e/cw.f and it.g headquarters described above. Men-only parenthesized HQ TOE is outside this vehicle-point house rule.

Headquarters with identified weapons retain each weapon's own rate and point count. First-line trucks are priced separately. A parent never adds another bill for its children's fuel. The house rule does not identify a vehicle model or add a source fuel_rate to the class records.

## Rationale
The sources require fuel use but leave the general HQ rate unspecified. Factor one is an explicit **HOUSE RULE chosen by the owner**, rather than a conclusion supported by those sources. It supplies a playable cost without pretending that an unknown equipment model has been transcribed. GAPS U-025 remains open as the permanent record of this source gap and the house-rule rate.

## Affected behaviour and tests
Rules-airlog owns the implementation and regressions for the adopted movement fuel contract. Required tests cover the nine affected scalar HQ records, absolute Under/Over counts, the same chart rows and rounding as truck and reconnaissance points, continued per-weapon rates, separate first-line truck billing, and no rebilling of attached children. No printed class consumption figure is invented. The earlier Unsupported-only proposal is superseded by this adoption.
