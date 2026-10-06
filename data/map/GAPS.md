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

- All 7023 terrain values are `unclassified`; zero terrain classifications verified.
- Hexside features and facilities remain undigitized. Their schemas are planned.
- TEC category/feature vocabularies must be pinned before classifications publish.
- `grid-preview.svg` is a section-colored geometry preview; terrain and feature
  artwork will be generated after data is verified.
