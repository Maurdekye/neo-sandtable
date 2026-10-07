# airlog-0014: Ready ammunition for one supported firing

- **Cases:** airlog:50.13, airlog:50.14, airlog:50.17, airlog:50.2
- **Status:** proposed
- **Profile version:** cna-2021-full / cna-2021-dev
- **Decided by:** neo-sandtable and rules-airlog, 2026-10-07
- **Owner review:** pending

## Question
How much ammunition can a unit hold itself when several combat functions have different consumption rates?

## Evidence
Case50.17 permits a unit's own reserve for one firing. Cases50.13 and50.14 price the chosen combat function using the participating TOE. Chart50.2 assigns different rates to those functions.

## Ruling
Calculate the cost of each supported single firing and use the largest as the ready-ammunition limit. Do not sum successive fires. An explicit weapon contributes only where its corresponding combat rating is positive. The close-assault infantry rate uses the source-verified unit classification; unresolved infantry identities and HQ composition remain unsupported.

## Rationale
This reserve can pay for any one function the unit can perform, while avoiding the capacity for a sequence of actions. The rule does not select a function when ammunition is loaded.

## Affected behaviour and tests
logistics::ready_ammo_capacity and distribution top-ups validate this limit. Tests cover ordinary, machine-gun and heavy-weapons infantry, weapon functions and unresolved classifications. Capacity uses current actual TOE, including understrength and overstrength units.
