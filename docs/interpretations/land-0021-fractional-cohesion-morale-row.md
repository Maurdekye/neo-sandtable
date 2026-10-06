# land-0021 - Fractional cohesion and the morale row

- **Cases:** land:17.4, land:17.22, land:17.24
- **Status:** proposed
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-07
- **Owner review:** pending

## Question
Quarter-point fatigue under land-0020 can leave a nonintegral cohesion value, while the morale
chart supplies only whole-number rows. Which row should a fractional value use?

## Evidence
Movement can cost a quarter CP; excess expenditure reduces cohesion by the same amount.
The morale chart lists successive integer levels, with open bands at +8 and -17.

## Ruling
Use the greatest integer no higher than signed cohesion to select the morale row. Thus -3.25
uses -4, while +3.75 uses +3. Keep the exact quarter value in unit state: this changes only the
lookup. Apply the chart's +8 and -17 endpoint limits after choosing the row.

## Rationale
The integer levels become consecutive bands rather than isolated points. A unit must attain
a higher cohesion level before it benefits from that row. Repeated quarter expenditures do
not change stored fatigue through rounding.

## Affected behaviour and tests
MoraleTable::modifier_quarters selects -4 for -13 quarters and +3 for +15 quarters. Tests compare
fractional and whole-row results, including the adopted -4 reading-56 gap fill.
