# air-0005 - Initial mixed-squadron pilots require an explicit training type

- **Cases:** airlog:35.21, airlog:35.24, airlog:40.12, airlog:40.14, scen:59.34
- **Status:** proposed
- **Profile version:** v0.1
- **Decided by:** rules-air, 2026-10-07
- **Owner review:** pending (consequential)

## Question
Initial pilots are assigned to squadrons and trained by aircraft type. A legal mixed-type squadron does not identify which type its starting rated pilots know.

## Evidence
35.21 permits more than one type within one aircraft class. 35.24 assigns pilots to the squadron, while 40.12 explains pilot training by assuming a squadron generally has one type. 40.14 requires time to learn another type. The initial scenario pilot roster supplies ratings rather than trained aircraft types.

## Ruling
For an initial squadron containing exactly one present aircraft type, import rated pilots as trained on that type. For a mixed squadron, leave the training type unresolved until its owning Air seat explicitly selects one of the present types for each rated pilot at initialization. This initial selection is provenance, not a mid-game reassignment, so it does not incur retraining. Unassigned force-pool pilots also remain unresolved until their initial assignment. Do not silently grant a pilot training on every type.

## Rationale
The player controls initial squadron/pilot assignment; the missing training fact should be an explicit setup choice. One type per pilot preserves the later type-training restriction without making arbitrary assignments based on map order.

## Affected behaviour and tests
Implemented foundation: air::inventory::initialize preserves unresolved training as None for mixed squadrons and force reserves. Test initial_pilot_training_is_known_only_for_a_single_present_type verifies import. The owning-seat training choice and flight validation remain to be implemented in the assignment slice.
