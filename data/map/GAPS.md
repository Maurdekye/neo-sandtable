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
  image hexes outside A-E zones (and Malta) are not in this grid. No land/sea flags
  have yet been classified. Completeness refers only to the VASSAL A-E mask.
- **Edge artifacts:** A includes partial column-00 hexes; E includes column 34.
  Confirm their playability and any off-map entry meanings against rules data.
- **Boxes:** current inventory is names only. No distances, capacities, routes or
  attachment hexes are inferred from placement or names in the module UI.

## Classification and rendering

- Pilot `graziani-0001` reviews 73 source-mask cells at C first35..42/second18..27.
  Published: 62 classifications (49 clear, 6 rough, 7 sea). Remaining: 6961
  unclassified cells, including the 11 coastal deferrals listed in the batch.
- **Coastal base terrain/domain:** the key has a coast symbol but no cost row.
  Mixed sea/land cells are C4221, C4122, C4021, C4022, C4026, C4027, C3922,
  C3923, C3924, C3925, C3926. Preserve observed `coastal` flags but leave the
  single base-terrain value unknown until the lead rules on representing land
  substrate versus a coast category. Particularly small land fragments and
  anchor/text symbols can confound raster rules. No water/land majority policy
  is implemented. The palette's automatic sea proposal for C4026 is quarantined.
- The proposal classifier covers only clear/rough/sea and abstains on ochre
  contour color. Desert, gravel, salt marsh, vegetation, mountain, delta, swamp
  and major-city classification methods remain to be validated.
- Hexside features and facilities remain undigitized. Their schemas are planned.
- TEC category/feature vocabularies must be pinned before classifications publish.
- `grid-preview.svg` remains a section-colored geometry preview;
  `terrain-preview.svg` adds the 62 reviewed terrain fills. Hexside and facility
  artwork remains pending.
