# airlog-0008: Mixed cargo shares truck capacity

- **Cases:** airlog:53.11, airlog:54.2
- **Status:** proposed
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** rules-airlog, 2026-10-07
- **Owner review:** pending

## Question
The truck chart gives separate carrying limits for each supply, but does not define the arithmetic for a mixed load on the same truck points.

## Evidence
Case53.11 assigns each first-line truck to personnel or supplies. The54.2 chart gives a carrying capacity per truck point and supply type. Those capacities are not interchangeable tonnage figures.

## Ruling
The player supplies a packing by light, medium and heavy truck type. Subtract trucks allocated to personnel. Within each remaining type, add the fractions occupied by each supply: points divided by that supply's printed capacity. The sum cannot exceed the remaining truck points. Evaluate the fractions with exact integers.

## Rationale
Independent maxima would let a truck carry several full loads simultaneously. A common tonnage limit would replace the printed capacities with an invented figure. Proportional fractions retain every printed single-supply limit while permitting mixed cargo.

## Affected behaviour and tests
Loading, redistribution and well draws validate the proposed final packing before accepting carried supplies. A half ammunition load plus a half fuel load fits one heavy truck; an additional water point does not. Trucks already motorizing a unit cannot also carry that cargo.
