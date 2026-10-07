# land-0020 ? Fractional CP and cohesion

- **Cases:** land:6.21, land:6.22, land:6.26, land:8.37
- **Status:** adopted
- **Profile version:** v1
- **Decided by:** rules-land, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 2)

## Question
The TEC permits half-point costs, and tracks can halve those again. Case 6.21 assigns one
Disorganization Point per excess CP but supplies no rule for rounding fractional excess.

## Evidence
The road entry price for vehicles is half a CP. The adopted track correction halves feature
prices. Cohesion decreases as excess expenditure is charged, rather than at the end of a Stage.

## Ruling
Preserve quarter CP and quarter cohesion points exactly. Charge the increase in excess CP,
with no rounding at a hex or answer boundary. A cohesion value of -26 points is -104 quarters.
Other procedures can give whole cohesion points by multiplying their values by four.

## Rationale
This preserves the stated one-to-one relationship and makes fatigue independent of whether an
AI seat submits a path at once or in several answers. It avoids charging a whole DP for every
half-point road entry.

## Affected behaviour and tests
Capability and movement accounting. Four quarter-CP actions beyond CPA have the same fatigue
as one whole-CP action. Cohesion -104 quarters prevents movement; -103 does not yet prevent it.
