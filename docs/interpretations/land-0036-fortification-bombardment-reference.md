# land-0036 - Fortification bombardment cross-reference

- **Cases:** land:25.14, land:12.51, land:12.53, airlog:39.37, airlog:41.37, airlog:41.5
- **Status:** adopted
- **Classification:** minor
- **Profile version:** v1
- **Decided by:** neo-sandtable via rules-mgr, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 3)
- **Lead approval:** approved as written by neo-sandtable via rules-mgr, 2026-10-07

## Question
Which bombardment case replaces the stale air reference in25.14, and does the air destruction limit also restrict artillery?

## Evidence
Case25.14 points to39.37 for damage to fortifications. That air case concerns fighter screening;41.37 is the fortification-damage procedure. Artillery facility attacks instead follow12.51-12.53 and their specified41.5 table column. A result against units does not establish damage to a fortification.

## Ruling
Read the stale reference as airlog41.37. Its one-level-per-Operations-Stage limit applies to air attacks only. Artillery changes fortification strength only when an explicitly selected facility target receives the corresponding cited facility damage result. No common artillery stage limit is inferred from the air restriction; any artillery limit must be sourced from its own table or notes.

## Rationale
This connects the cross-reference to the actual air procedure while retaining the distinct artillery facility procedure. The lead approved this narrow correction and air-only limit at08:50 through rules-mgr.

## Affected behaviour and tests
The generic engineering reduction performs one saturating level change and retains zero overrides. Source-specific callers enforce their own damage permission and quota; no automatic shared quota is added. Planned tests cover the corrected reference, two air outcomes at one site in the same stage reducing at most one level, next-stage air eligibility, independent cited artillery outcomes without an invented air limit, no reduction from unit loss/pin results, and persistent zero city overrides. These procedure tests are planned, not implemented.