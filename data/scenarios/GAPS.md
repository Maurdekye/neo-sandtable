# Scenarios — gaps and open questions

Format: `S-NNN` · area · what is missing or unclear · source checked · status.

- **S-001** · `scen:60.45` · The printed Alexandria/Valletta fleet roster is struck through in the 2021 booklet; only the totals in the first paragraph (1 battleship, 3 cruisers, 7 destroyers) are live text, and the Commonwealth Fleet schedule (`land:30.6`) governs deployment. The totals are recorded, not the struck roster. · open
- **S-002** · hex ids · Several hexes in the retyped booklet text are OCR-fragile; ids were read from the booklet page images (pp. 27–28). Verify against `data/map` once available. · open
- **S-003** · `scen:60.31` "3 CCNN Div": the sheet has no divisional HQ; see interpretation scen-0004. · proposed
- **S-004** · `scen:60.43` Commonwealth second-third line trucks: the location column of the third row reads "Anywhere, maps" (apparently a typo for a list of map sections). Recorded with area `unclear_anywhere_maps` (15 light, 40 medium, 5 heavy). Also the Commonwealth supply table (`scen:60.44`) has no water column: no initial Commonwealth water is listed. · open
- **S-005** · `scen:60.5` the air-facility tables have unnamed rows printed with a dash: one airfield (B5825) and five landing strips (C4119, D3231, D3416, D3516, D3903). Entered without names. · open
- **S-006** · hex ids: Benghazi — see interpretation scen-0001. Off-map facilities print only `E(1833)`, `E(3433)`, `E4033` or "Off-Map" (Fayid and Ismailia have no id); off-map facilities now carry the cartographer's `location` ids (data/map/areas.toml). The shared printed token E(1833) (Deversoir, Kabrit) is disambiguated by id. · resolved
- **S-007** · `scen:60.31` Autoblinda 40 points: the booklet says "ID Code WW" for the recce points; the class table has `ww` (45 CPA, Autoblinda 40) but no weapon-system entry exists for them; stored as `recce_toe_points` with class `it.ww`. · noted
- **S-008** · `scen:60.45` struck-through text (see S-001), including the struck "no fleet units may be moved until September 1/IV" sentence; only the live "second Game-Turn, first OpStage" restriction is recorded. · noted
- **S-009** · `scen:60.41` "Broken Down Vehicles": recorded at Alexandria as 2 TOE of A9 and 1 TOE of A10 cruiser tanks; the booklet does not say how they are repaired or whether they count against any unit's maximum TOE (assumed handled by the repair rules, `land:22`). · open
- **S-010** · `scen:60.34` Axis dumps for Benghazi and Tripoli (box) have no hex printed; placed by city name. · noted
- **S-011** · `scen:60.81` the printed Italian "Tactical Victory" wording ("retain possession of Sollum ... and Fort Maddalena and Giarabub in supply") is ambiguous on whether the supply requirement applies to Sollum; recorded as retain-all with supply to Tobruk. · open
