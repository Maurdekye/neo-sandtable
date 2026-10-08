# land-0038 - Holdings aboard repaired and towed trucks

- **Cases:** land:21.43, land:21.64, land:21.65, land:22.5, land:20.2, land:20.4, land:3.62
- **Status:** adopted
- **Profile version:** cna-full-v1 / cna-dev-v1; runtime integration not implemented
- **Decided by:** neo-sandtable, 2026-10-07 (recovery design decision)
- **Owner review:** approved 2026-10-08 (batch review 4)
- **Consequential:** yes

## Question

How do cargo, embarked passengers and reserves follow repaired trucks or a partially towed marker, without repair or towing resetting their physical and movement accounting?

## Evidence

land:21.43 associates holdings with broken trucks and permits working trucks to take them only when capacity exists. land:21.64 permits towing a selected portion and requires a separate marker. land:21.65 exempts towing from supply charges and breakdown. land:22.5 returns repaired equipment through the replacement procedures and requires fuel for later movement. These cases do not specify a digital partition of unassigned holdings across random repair outcomes or a relationship between a marker's tow budget and existing cargo/truck histories.

The lead's 2026-10-07 recovery design decision resolves those representation questions. The owner approved this reading in batch review 4 on 2026-10-08. This record documents decisions 2, 3 and 6; it grants no new shared State or helper implementation permission.

## Ruling

Cargo follows repaired trucks into the owner's actual pool at the repair site without unloading or handling CP. Partial repair uses the owner's cargo-lot priority submitted before any die, with the baseline ordering stable cargo-lot ids. Finish validates the merged pool's total capacity and leaves unallocated cargo on the marker. There is no post-roll allocation or destination switch.

Passengers never enter a pool. They remain on the marker, even with no trucks left, until the adopted land-0028 dismount or collection procedure applies. This stationary passenger accounting grants neither free travel nor vehicle capacity.

Activity water aboard repaired trucks follows them. Vehicle tank fuel and non-truck crew reserves remain on the marker only within the capacity of its unrepaired equipment; excess is lost at repair and reported privately. Repaired vehicles require fresh fuel for later movement under land:22.5. Former payment never supplies new spendable credit.

A partial tow names exact type/cohort counts and the accompanying cargo. Finish validates capacity and creates the new marker while retaining the exact remainder. Towing uses the marker's separate Repair-Phase budget. It does not advance, erase or replace physical truck or cargo-lot CP histories. Existing history remains intact; missing history remains Unknown.

The lead's superseding R5/R6 ruling removes the earlier zero-body reserve-retention and pickup proposal. Zero remaining vehicles can retain passengers awaiting normal dismount; no reserve pickup entitlement or new capacity is inferred.

Repair/tow provenance Notes go only to the acting side, using Audience::Side(actor) and ordinary GameEvent::Note. Never notify the original payer or former owner about the captor's activity. A captured-equipment Note contains only the captor's physical repair/return/fuel-loss facts; former accounts, charges and sources are dropped. The operator receives its usual copy, and central Sync remains unchanged.

## Rationale

The partition conserves holdings and respects verified truck capacity. Choosing priorities before dice preserves the rolled result and removes the need for a hidden inventory-dependent post-roll decision. Separate tow accounting respects its special supply exemption without inventing truck or cargo travel credit.

## Affected behaviour and tests

No runtime implementation is claimed. The future repair/split-tow finish procedures require all/partial repair and junk tests, merged aggregate capacity, default and owner cargo priorities, passengers retained until ordinary dismount, no handling debit, capped reserve/private loss and insufficient-capacity remainder tests. Tow tests compare Known and Unknown truck/lot histories before and after relocation. Actual Respond/Advance, private-view pairs, atomic later-failure rollback, exact retry and checkpoint recovery must preserve outcomes and issue no duplicate equipment or credits. A paired test must show that captor repair/tow leaves the former owner's stream unchanged and exposes no former funding details to the captor. The revision-7 fields/signatures are lead-approved. This interpretation is adopted; runtime implementation and its required proofs remain separately scoped.
