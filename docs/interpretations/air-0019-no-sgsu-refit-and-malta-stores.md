# air-0019 - No-SGSU refit groups and Malta Stores

- **Cases:** airlog:36.5, airlog:38.33, airlog:38.34, airlog:38.35, airlog:38.36, airlog:38.39, airlog:43.21, airlog:43.22, airlog:44.11, airlog:44.12, airlog:44.14, airlog:44.16
- **Status:** proposed (lead-decided; consequential owner review pending)
- **Profile version:** intended cna-2021-dev/full maintenance extension; existing missing-procedure guards remain
- **Decided by:** neo-sandtable, 2026-10-14, disposition ma04b424094d3
- **Owner review:** pending; this file must be on main before entering an owner batch

## Question

German Mediterranean bombers and Malta aircraft refit without SGSUs. Which supply exceptions, equivalent-squadron groups, modifiers and limits apply? This interpretation records the lead's disposition of the source/API proposal without implementing the later no-SGSU consumers or Malta loading producer.

## Evidence

36.5 provides several off-map facility exceptions, including unlimited aircraft maintenance and repair supplies. Its final Mediterranean qualification adds bombing immunity to those bases, linking them to the preceding facility regime.43.21 removes SGSUs and player fuel/ammunition spending for German bombers based in Italy, Sicily and Crete, but still requires refit.43.22 specifies groups of6-12 as equivalent squadrons for that method.

Malta's permanent facilities are defined separately in44.12.44.14 removes SGSUs and gives each facility level an18-plane handling capacity, with squadron-like groups for bookkeeping.44.16 supplies fuel and ammunition automatically but retains normal refit. It does not itself grant free Stores.38.36 prices attempted squadron refit whether it succeeds or fails.

38.34/.35/.39 provide the ordinary refit methods and aircraft-nationality/foreign-SGSU modifiers. A nonexistent SGSU supplies no foreign-assignment relationship. Adopted airlog-0022 already limits attempts by period and distinguishes genuine36.5 aircraft Stores entitlement from SGSU dues and fuel/ammo-only rights. Adopted airlog-0023 supplies predeclared successful-id priority.

## Ruling

1. Italy, Sicily and Crete boxes are36.5 off-map air facilities. Their aircraft refit Stores requirements are satisfied by36.5a unlimited maintenance supplies.43.21 additionally removes SGSUs and player fuel/ammunition expenditure.43.21's fuel/ammo clause alone is not the Stores grant.
2. Malta is a separate section44 regime, not a36.5 facility. Its refit Stores remain finite and must be drawn from its actual stocks through a reviewed no-SGSU facility consumer.44.16 supplies only automatic fuel/ammunition. No free Stores, fictional dump or initial load credit follows from an off-map location or missing metadata.
3. No-SGSU refit groups are logical squadrons. Use aircraft-nationality modifiers and the normal bound refit methods; apply no foreign-SGSU penalty where no SGSU exists. Use airlog-0023 priorities and actual per-plane identities as appropriate.
4. The owner partitions all German bombers in each Mediterranean box into6-12-plane groups, frozen before dice for the maintenance period. One group of1-5 is allowed only when that whole box holds fewer than6 bombers. Each group records one logical Stores requirement per period, supplied by36.5a. Do not regroup to create extra percentage ceilings or cheaper service.
5. On Malta,18 times the current individual facility level limits both based aircraft and attempted PlaneIds per refit period; level0 allows none. Logical groups are frozen per period and cost one finite Stores each under38.36, regardless of success. Moving groups or aircraft does not create another same-period attempt or reduce the price.
6. Exact44.16 automatic refuel/rearm stage timing and load choice remain deferred to the Malta producer design. This interpretation chooses no public cursor anchor or unrecorded mode/payload.

The approved first implementation slice is narrower: genuine36.5 refit Stores for actual SGSUs on the six verified Commonwealth and four Tripoli/Tunisia source pairs, with finite35.14 operation dues. No later no-SGSU grouping, supply facade or Malta producer is activated by that slice.

## Rationale

The Mediterranean bombing qualifier places those bases within36.5, providing independent Stores authority consistent with adopted airlog-0022. Malta's separately printed facility regime and specifically enumerated automatic supplies do not supply equivalent Stores authority. The no-SGSU modifier reading avoids inventing an assigned SGSU, while retaining printed nationality effects.

Freezing real logical groups preserves the squadron price and prevents plan partitioning from changing success arithmetic. The narrow understrength exception handles a box with fewer than6 actual bombers without inventing aircraft or allowing arbitrary undersized groups in a larger force. Malta's explicit plane-per-level capacity supplies its physical and period budget rather than an invented SGSU capacity.

## Affected behaviour and tests

Future source-ready no-SGSU consumers must pin Mediterranean source boxes/German bomber eligibility,6/12 groups and balanced larger partitions, the whole-box1-5 exception, immutable membership, exact nationality/assignment modifiers and one logical Stores charge. Malta must pin actual individual sites/private initial capacity, finite Stores conservation,18*current-level occupancy and attempt budgets, zero capacity, frozen groups and same-period moved-aircraft duplication.

Canonical Mediterranean box sites, individual Malta facility identities and private initial distribution remain owning content gaps. Do not invent IDs, capacities, coordinates, stock or SGSUs. The automatic44.16 producer requires its own reviewed stage/load contract. Full maintenance remains Unsupported for still-missing procedures. Both later slices require Respond purity, whole-State/RNG/events rollback, checkpoint/reentry and testkit hidden-fact/ordered-stream pairs.

This file records a consequential lead ruling pending the owner's batch review. It does not mark the no-SGSU implementation complete, publish a rules-profile version or transfer earlier finite validation proof to new code.
