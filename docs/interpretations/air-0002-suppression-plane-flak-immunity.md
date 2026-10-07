# air-0002 - Suppression planes are excluded from AA target groups

- **Cases:** airlog:40.72, airlog:40.74, airlog:40.75, airlog:46.24
- **Status:** proposed
- **Profile version:** v0.1
- **Decided by:** rules-air, 2026-10-07
- **Owner review:** pending (consequential)

## Question
One flak-suppression clause permits remaining AA to attack suppression fighters, but the later AA restrictions explicitly exclude those fighters.

## Evidence
40.72 allows unsuppressed AA to attack any aircraft at the target, including suppression fighters. 46.24 specifically exempts fighters carrying out suppression and convoy recon planes from AA fire. 40.75 still requires ammunition use by suppression fighters and suppressed guns.

## Ruling
Apply the explicit exclusion in 46.24: do not place suppression aircraft in AA target groups. Apply suppression only to light or ship AA; remaining AA attacks eligible other groups. Both suppression fighters and neutralized AA consume the ammunition required by 40.75.

## Rationale
The dedicated AA restriction is narrower than the general suppression description. This choice changes aircraft exposure and therefore requires owner review.

## Affected behaviour and tests
Planned: suppression immunity, heavy-AA survival, eligible other-group fire, and suppressed-AA ammunition tests. Not operational in the inventory foundation.
