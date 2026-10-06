# Italian artillery arrival AA loadouts versus class capacity

- **Cases:** land:4.45, land:4.46b
- **Status:** proposed
- **Profile version:** draft
- **Decided by:** oob, 2026-10-07
- **Owner review:** pending

## Question
How should the historical OA loadout be represented when its AA points exceed the reusable class capacity?

## Evidence
The Sirte 43rd and Ariete 132nd artillery regiments both have code kk on their OA sheets and each lists six light AA points. The Italian characteristics chart permits nine artillery points plus at most three light or heavy AA points for kk. The images are legible; this is a source conflict rather than an unreadable digit.

## Ruling
Retain six light AA points in each explicit OA arrival loadout and retain the class's printed AA capacity of three. Loading data must not discard three points or expand the generic capacity silently. Until the rules profile resolves the two named units' overstrength status, any adjudication depending on that capacity conflict must remain explicitly unsupported.

## Rationale
Both charts supply numerical facts. Changing either would invent an unstated correction. An arrival loadout and a class limit are separate records, so the conflict can remain visible without rewriting either source value.

## Affected behaviour and tests
The two OA weapon lists retain n=6 for it.aa_light_20mm; class it.kk keeps max_toe_extra=3 and anti_air max=3. validate.py and coverage.py check references and arrivals, not a capacity exception. No implicit bypass is implemented. Engine capacity adjudication and the eventual profile ruling are for the lead and rules clerks.
