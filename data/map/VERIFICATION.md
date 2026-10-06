# Verification: geometry and terrain pilot

Date: 2026-10-06. Profile: `vassal-2021`. Build-definition SHA256:
`bd50ebff16fb2704fbfb28235a7e99acc5c6c5da6cb84bffcfa283659b9780ac`.
The inspected image is the local 2021 re-rendering (14310 x 4632 pixels), not
original 1979 sheet art. No inspection images enter the repository.

## Automated geometry checks

- Exhaustive source-zone enumeration: 7055 id locations, 7023 unique axial cells,
  32 duplicate ids. No other collisions; registration residuals bounded during
  generation to 15 pixels horizontally and 3 vertically against the continuous
  inspection lattice. These bounds describe section registration, not terrain error.
- All 7023 canonical ids round-trip through the independent published-data lookup.
- All neighbour links have distance one, reciprocal opposite directions, and
  continuity across each of A-B, B-C, C-D and D-E. Invalid or absent ids fail.
- Six scenario anchors have pinned expected axial values. All 32 observed aliases
  resolve to their canonical coordinates; synthetic alias plumbing is also checked.
  The 32-hex flight in airlog:34.11 and adjacency in land:15.35 agree with this grid.
- The extractor reads numeric geometry, not raster pixels. Its polygon mask
  calculation is not an independent audit of every drawn hex in the image.

## Local visual inspection performed

A scratch contact sheet placed generated center marks over 26 crop locations:
A1816, A2021, A2629, A4130, A4829, A4827, B5504, B5925, B4921, C4807, C4321,
C4021, C4131, C4218, C4120, C1014, C0127, D3714, E3613, E3714, E1430,
A2109, E3815, E4019, B5810, B5809.

Twenty-two distinct named landmarks match: El Agheila, Mersa Brega, Agadabia,
Soluch, Benina, Benghazi (id still text-ambiguous), Barce, Derna, Mechili, Tobruk,
Bardia, Sollum, Sidi Barrani, Sidi Azeiz, Giarabub, Siwa, Mersa Matruh,
Alexandria, Helwan, Marble Arch, Aboukir and Rosetta. Alexandria's two checked
hexes represent one landmark. The remaining crops are C4120's setup position
and the adjacent B5810/B5809 wadi/slope example (land:15.35). Marble Arch is
an independent land:8.19 anchor; Aboukir/Rosetta are airlog:55.21 anchors.

No checked marker lay in a different drawn hex from its associated landmark.
Some blue place dots lie off-center; matching means same hex, not coincident dot.
Observed mismatch rate: **0/22 named-landmark checks**, including the visually
inferred A4827 Benghazi candidate. Excluding that unresolved text reading: 0/21.
This convenience sample is not a random accuracy estimate or a claim of zero
error over the full map. The incorrect B4827 reading was explicitly rejected.

Eight seam windows (four section joins, at map y=2500 and y=4200) were viewed
with labeled generated centers over the local image. They showed continuous
adjacency and centers inside their corresponding drawn hexes. These windows
provide a qualitative topology check; no per-hex error rate is inferred.

## Reproduce the local overlays

`tools/map/verify_overlay.py --output <outside-repo-folder>` writes the contact
sheet and eight seam windows. It requires Pillow in the inspection environment
and `CNA_SOURCES`. Local output from this initial check lives in the cartographer
scratch `runs/` directory, outside the clone. Do not publish those source images.

## Limits and next audit

**Terrain pilot: 62 classified records, single visual pass; full-map error rate
unknown. Hexside/facility verification: zero classified records.** The original
grid preview remains neutral; the separate terrain preview renders the pilot. Original sheet
seams remain unresolved. The complete first acceptance condition is not met
merely by this module-profile geometry milestone.

Next: resolve coastal base-terrain semantics and the Benghazi token; expand the
reviewed terrain area on C and D. For future terrain results, report both
sample selection and category-wise error counts; ambiguous cells remain gaps.
If original 1979 map sheets arrive, compare a stratified sample by section and
terrain, all ambiguous cells, directional hexsides and referenced facilities,
and record differences rather than treating the 2021 re-rendering as identical.


## Terrain pilot graziani-0001 (2026-10-06)

Exact local image SHA256:
`904c884d0933e6dc21599243b038a4364d46a5f2151ac9bc8eda852d5c9b6011`.
TEC vocabulary was read from the double-verified `data/tables/land/8.37-terrain-effects.toml`.
The review record is `data/map/reviews/graziani-0001.toml`.

Selection: C first-axis35..42, second-axis18..27. Eighty possible rectangular
ids reduce to 73 actual mask members. All 73 were inspected individually against
a local contact sheet at source scale, including their drawn boundaries and
nearby symbols. The whole small window was reviewed; no uninspected proposal was
published. Source crops remain outside the repository.

| Review outcome | Count |
|---|---:|
| Accepted clear | 49 |
| Accepted rough | 6 |
| Accepted solid sea | 7 |
| Deferred mixed coast | 11 |
| Total examined | 73 |

The palette-v0.1 proposal algorithm recognizes measured solid clear/rough/sea
colors; ochre in the center triggers an abstention because a contour splash can
look like mountain fill. Compared with the single visual pass, 58 non-abstaining
proposals among the 62 accepted records matched; **0/58 disagreements** observed.
The other four accepted records were abstentions: C4218/C4219/C4121 are clear
under contour symbols, C4123 is sea obscured by blue text. This is performance
against one observer's review, not an independent estimate of classification
accuracy. The 11 unresolved coast cells (including 7 non-abstaining proposals)
are excluded from that denominator. They do not count as validated successes.

No classification from this pilot is claimed to have a second visual review or
to have been verified against original 1979 sheets. True post-review error rate
is unknown. Full-map coverage is 62/7023 (under 1%); 6961 terrain values remain
unknown. Flags cover only the reviewed surface-domain layer; roads, railways,
contours, ports, villages, wells, fortifications and other facility layers remain
unverified even where a base terrain has been accepted.

Replay hashes the exact local image and build definition, checks observer/date
and review states, rejects duplicate/invalid ids and unknown TEC classes, then
rebuilds accepted terrain and explicit coastal deferrals. Changes to source
identity require re-review. Tests exercise these failure paths and ensure the
published values/citations correspond to accepted decisions. Reproduce the
proposal sheet with the command in README, and replay with apply_terrain.py.
