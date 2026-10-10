# Headquarters activity water when equipment is unspecified

- **Cases:** airlog:52.41, airlog:52.42, airlog:52.43
- **Status:** adopted
- **Profile version:** cna-2021-dev / cna-2021-full
- **Decided by:** the owner, 2026-10-09 (question qff6681c4edd2)
- **Owner review:** reviewed 2026-10-09; HOUSE RULE chosen by the owner, relayed by neo-sandtable in m32aa7e58cadc

## Question
How much activity water does a headquarters with an unparenthesized numeric TOE but no identified equipment need before using CPA?

## Evidence
Airlog:52.41 distinguishes infantry body water from vehicle activity water. Airlog:52.42 charges vehicle and truck points for an OpStage with CPA use; airlog:52.43 supplies the heat multiplier. The scalar HQ counts do not identify equipment. The existing units-0005 house rule supplies movement fuel for these points so the headquarters can move, while the unresolved water composition has prevented that movement. GAPS U-025 remains the record of the missing printed composition.

## Ruling
This is a **HOUSE RULE chosen by the owner**, extending units-0005 to activity water; it is not a claim about a printed HQ equipment model. Use the same canonical classifier: headquarters class, unparenthesized maximum TOE, and current Normal, Under or Over rather than Weapons. Each current HQ TOE point needs one activity-water point for an OpStage in which it uses any CPA, with the ordinary airlog:52.43 heat multiplier. Normal resolves to the recorded class maximum; Under and Over contain actual counts and retain canonical validity checks.

The nine current affected records are the six Commonwealth cw.e/cw.f and three Italian it.g headquarters covered by units-0005. First-line truck points have their own additional water obligation. Parenthesized men-only headquarters and headquarters with explicit weapon lists keep their existing paths. Attached children's obligations are not billed again to the parent. No equipment, source rate, strength fallback or new ledger field is invented.

## Rationale
The owner chose vehicle-point water to make the intended HQ movement possible without granting water-free activity. The existing activity ledger retains the per-OpStage payment, heat, truck-credit and shortage rules. Missing or invalid strength remains an error rather than zero demand.

## Affected behaviour and tests
Activity demand reuses the units-0005 classifier and canonical strength accessor. Tests cover all nine records, Normal/Under/Over, heat, separate first-line trucks, dry atomic refusal, repeated CPA payment and the next OpStage. Movement tests preserve unknown terrain coverage and check watered movement and dry water refusal in both profiles. Equipment, men-only and genuine missing-source controls remain independent.
