# land-0029 - Light truck track breakdown example arithmetic

- **Cases:** land:8.37, airlog:54.2
- **Status:** adopted
- **Profile version:** cna-full-v1 / cna-dev-v1
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2)

## Question
The light-truck footnote on 54.2 gives 18 BP for a track crossing a wadi into rough terrain, although its stated factors and the corrected Terrain Effects Chart give 10. Which value applies?

## Evidence
The 54.2 footnote adds one BP for an off-road hex entry and one for crossing a feature. A wadi and rough terrain each give eight BP before track reduction. Under adopted land-0002, the track halves each base value. Thus the contributions are four, four, one and one: ten BP. The printed worked example reports eighteen instead.

## Ruling
Apply the corrected terrain values and the footnote's extra-point instruction. The example's total does not override them: this crossing costs ten BP for light trucks. The extra points themselves are not halved. Keep the source discrepancy documented.

## Rationale
The instruction and corrected chart specify the calculation; the example is an inconsistent illustration. This follows the instruction-over-example approach used in airlog-0005 and confirms land-0002.

## Affected behaviour and tests
Light trucks retain a separate BP history from other vehicles. Track through rough with a wadi contributes eight base BP plus two light-truck BP. Road travel has no light-truck extra; two off-road features add three BP including the entered hex.
