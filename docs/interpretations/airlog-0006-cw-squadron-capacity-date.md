# airlog-0006 — When Commonwealth squadron capacity increases

- **Cases:** airlog:35.23 (Squadron Capacity Chart)
- **Status:** proposed
- **Profile version:** v0.1
- **Decided by:** rules-airlog, 2026-10-06
- **Owner review:** pending

## Question
The text of 35.23 says Commonwealth squadrons increase capacity starting with July 1941. The chart rows are
labelled "1940-41" (12 ready) and "1942-43" (18 ready), so it is unclear which capacity applies from July
to December 1941.

## Evidence
The text names an exact month; the chart labels are calendar-year ranges that do not break at July.

## Ruling
Use the larger capacity (18 ready / 6 reserve) from the first Game-Turn of July 1941, and the smaller
before that. The change is data-driven by the date in the rules profile.

## Rationale
The text gives a specific date; chart labels are looser. The Graziani window (autumn 1940) is
unaffected either way.

## Affected behaviour and tests
Squadron capacity lookup (`airlog.35.23.squadron_capacity`). Test: a CW squadron in Game-Turn of July
1941 has ready capacity 18.
