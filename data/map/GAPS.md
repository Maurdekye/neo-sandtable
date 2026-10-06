# Map gaps

## Geometry and source authority

- **Original seams (`land:4.1`):** original 01xx/39xx overlap conflicts with the
  VASSAL 2021 numbering and roughly 33 horizontal columns. The proposed profile
  preserves scenario ids and actual module duplicates; it does not claim to
  resolve original sheet seam aliases. Do not mark original-map coverage complete.
- **1979 map unavailable:** sources currently include original rulebook page
  scans, not original map sheets. The local PNG is a 2021 re-rendering. Its map
  details may differ. If original sheets become available, audit each section's
  terrain, coastline/boundary cells and every hexside/facility using a stratified
  sample before asserting correspondence to 1979; retain discrepancy records.
- **Benghazi id (`scen:60.31`):** extracted text has `84827`. B4827 falls inland
  in section B and cannot be Benghazi. The re-rendered city lies in A4827.
  This likely concerns text extraction, but checking the page is needed before
  publishing a corrected place id. Geometry includes both valid hexes and does
  not silently rewrite scenario text.
- **Water mask:** zone-contained centers include a narrow sea fringe; offshore
  image hexes outside A-E zones (and Malta) are not in this grid. Surface-domain flags
  exist only for the 143 reviewed cells; the rest are unknown. Completeness refers only to the VASSAL A-E mask.
- **Edge artifacts:** A includes partial column-00 hexes; E includes column 34.
  Confirm their playability and any off-map entry meanings against rules data.
- **Boxes:** current inventory is names only. No distances, capacities, routes or
  attachment hexes are inferred from placement or names in the module UI.

## Classification and rendering

- Three review batches cover 143 distinct cells: 142 classified (123 clear,
  12 rough, seven sea); 6881 terrain values remain unknown.
- **C4026 coastal fragment:** map-0002 resolves coastal semantics. Ten original
  coastal deferrals are now classified by land substrate. C4026 retains the
  coastal flag and unknown terrain because its tiny land fragment is unreadable.
  Its automatic sea proposal is rejected; majority color cannot erase land.
- **Sollum port:** printed anchor and label identify C4022. Published place
  `port-sollum` has no verified capacity or other attributes. The town dot is in
  C4021; no town record is silently inferred from the port's hex.
- The proposal classifier covers only clear/rough/sea and abstains on ochre
  contour color. Desert, gravel, salt marsh, vegetation, mountain, delta, swamp
  and major-city classification methods remain to be validated.
- Hexside features remain undigitized, including all_sea edges across bays.
  Only one port is recorded; all other facilities remain pending.
- Base TEC vocabulary is pinned; feature extraction still needs local verification.
- `grid-preview.svg` remains a section-colored geometry preview;
  `terrain-preview.svg` adds the 142 reviewed terrain fills and a generic port marker. Hexside and facility
  artwork remains pending.
