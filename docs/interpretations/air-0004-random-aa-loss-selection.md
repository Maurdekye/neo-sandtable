# air-0004 - AA casualties use uniform sampling without replacement

- **Cases:** airlog:46.0, airlog:46.25, airlog:46.26
- **Status:** adopted
- **Profile version:** v0.1
- **Decided by:** rules-air, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 3)

## Question
AA results specify a count but leave the consistent mutually agreed method for selecting affected individual planes to the players. The digital edition needs one pinned deterministic RNG procedure.

## Evidence
46.0 requires random casualty identities within the target group and presents possible methods. Non-fighter losses precede selecting the involuntary aborts, so a destroyed aircraft cannot also absorb an abort.

## Ruling
Use campaign-RNG uniform sampling without replacement over the sorted eligible individual aircraft IDs within each target group. Cap count by surviving group size. Sample destroyed aircraft first, then aborts from remaining aircraft. Sampling uses rejection rather than modulo reduction when the candidate count does not divide the draw space. Record required random draws privately; publish only rule-required aircraft/group results with fresh disclosure labels.

## Rationale
One procedure provides equal individual exposure, replay stability and no repeated casualty. Sorting is a reproducibility mechanism, not a preference for any aircraft or pilot. A pinned method is necessary because the printed rule allows a table agreement.

## Affected behaviour and tests
Planned: selection count/uniqueness, exact small-domain sampling boundaries, destroyed-before-aborted ordering, replay and paired hidden-pilot tests. No random sampling is part of inventory import.
