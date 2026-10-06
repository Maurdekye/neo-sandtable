# land-0002 — Terrain Effects Chart: corrected footnotes and row naming

- **Cases:** land:8.37, errata79:8.37, land:8.46
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-06
- **Owner review:** reviewed 2026-10-06 by the owner (batch review 1)

## Question
The 1979 chart attached footnote 4 (Alexandria/Cairo are Level Three fortified, other cities Level Two) to Swamp and printed 1 CP for tracks; the errata moves footnote 4 to Major City and says footnote 8 applies to tracks. Also: the table row is called Gravel while the terrain key calls the class Rock/Gravel.

## Evidence
- Errata (8.37): footnote 4 belongs with Major City; track entry is not 1 CP but "halve the cost of the terrain in the hex" (footnote 8, which also excludes the CP and Breakdown of moving a vehicle down an escarpment).
- The VASSAL chart image already carries both corrections. Case 8.46 states tracks cost 1 CP per hex, which conflicts with footnote 8 read literally.

## Ruling
Use the corrected chart as transcribed in data/tables/land/8.37. A track halves the terrain cost of the hex it runs through (footnote 8), except for the CP and Breakdown of moving a vehicle down an escarpment. The track wording in 8.46 ("1 CP per hex") is exact only for Clear terrain (cost 2, halved to 1); for harsher terrain the footnote 8 reading governs, and the 8.46 wording is flagged for owner review. The row named Gravel is the Rock/Gravel hex class.

## Rationale
Errata and chart are the more specific authority; 8.46's 1 CP is exact only for clear terrain.

## Affected behaviour and tests
Movement cost function; Tests: track through Clear = 1 CP (motorized and not), track through Rough = half of the printed Rough cost, down-escarpment crossing by track unaffected by halving.
