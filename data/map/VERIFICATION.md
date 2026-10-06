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

**Terrain coverage: 142 classified records; full-map error rate unknown.
Hexside verification: zero records. Facilities: one visually checked port.** The original
grid preview remains neutral; the separate terrain preview renders the pilot. Original sheet
seams remain unresolved. The complete first acceptance condition is not met
merely by this module-profile geometry milestone.

Coastal semantics are adopted in map-0002. Next: resolve the Benghazi token and
C4026 fragment; expand the reviewed terrain area on C and D. For future terrain results, report both
sample selection and category-wise error counts; ambiguous cells remain gaps.
If original 1979 map sheets arrive, compare a stratified sample by section and
terrain, all ambiguous cells, directional hexsides and referenced facilities,
and record differences rather than treating the 2021 re-rendering as identical.


## Historical initial terrain pilot graziani-0001 (2026-10-06)

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

## Coastal amendment and inland expansion (2026-10-06)

Current coverage is **142/7023 classified**: 123 clear, 12 rough, seven sea;
6881 remain unclassified. Three batches cover 143 distinct cells. This remains
a convenience window, not a random sample or a whole-map accuracy estimate.

`graziani-0002` explicitly supersedes only the 11 previously deferred cells
under adopted map-0002. All were visually rechecked using the original native
scale sheet. Ten land substrates are readable: nine clear and one rough
(C4221). C4026 has visible land but an unreadable substrate; it remains unknown
and coastal. Six non-abstaining accepted proposals match these ten decisions;
four were abstentions. C4026's incorrect automatic sea proposal is still
excluded as an unresolved substrate, with its water-domain error documented.
The amendment is by the same observer, not an independent second review.

`graziani-0003` covers C first28..34/second18..27: all 70 cells inspected in the
local native-scale contact sheet, 65 clear and five rough (C3124, C3125, C3024,
C2922, C2822). All 70 non-abstaining proposals match that single visual pass:
0/70 disagreements. No original-sheet check or independent observer is claimed.
Contour marks in C3427 were distinguished from base fill; small off-cell rough
patches near C2823 were not assigned to its terrain.

Sollum's printed port anchor and associated label were checked in C4022. The
nearby town dot is C4021 and is kept distinct. `places.toml` records only the
port location and type, no unknown capacity or other attributes. This is one
visual facility review, not completion of the coast's port inventory. Its SVG
marker is a generic circle at the axial center, with no traced source symbol.

Replay requires explicit targeted amendments, rejects conflicting sea/coastal
flags, validates ports on coastal cells, and reproduces places.toml as well as
the five earlier generated files. All inspection images stay outside the repo.
