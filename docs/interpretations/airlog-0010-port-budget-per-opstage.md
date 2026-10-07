# airlog-0010: Shared port supply budget per OpStage

- **Cases:** airlog:55.14, airlog:55.16, airlog:55.3, land:30.57
- **Status:** proposed
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** rules-airlog, 2026-10-07
- **Owner review:** pending

## Question
The general capacity paragraph refers to a Game-Turn, while the chart, worked example and personnel adjustment use an OpStage. Is inbound and outbound supply capacity separate?

## Evidence
The55.3 chart assigns its supply limit to an OpStage and includes shipments in both directions. Case55.14 calculates Benghazi's reduced capacity for an OpStage. Case30.57 reduces a supply allowance during the stage of a planned personnel shipment. Case55.16 instead mentions a Game-Turn.

## Ruling
Use one supply tonnage budget per port per OpStage, shared by inbound and outbound shipments. Efficiency adjusts that budget before shipments are admitted. Mandatory scheduled reinforcements remain exempt under55.15.

## Rationale
The explicit chart period, numerical example and personnel procedure agree. Separate inbound and outbound budgets would double the chart allowance.

## Affected behaviour and tests
Naval arrivals and coastal loading/unloading use logistics::ports::charge. Tests combine inbound/outbound weights in one stage, reject overflow, and reset only at the next stage.
