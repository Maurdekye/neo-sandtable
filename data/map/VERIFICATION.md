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

**Terrain coverage: 227 classified records; full-map error rate unknown.
Hexside verification: zero records. Places: one visually checked port and five Cairo city cells.** The original
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

## Historical coastal amendment and inland expansion (2026-10-06)

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

## Historical initial area selectors and symbolic locations (2026-10-06)

31 areas and 13 distinct off-map locations are published as incomplete data.
Section sets replay against all canonical records plus alias memberships.
Tests pin 1261 members on section D (including canonical C4233 through D4200),
2558 in D/E and 2966 in A/B. Union membership is deduplicated. Seven tests check
seam membership, explicit failures for missing national sets/dynamic facilities,
four-box grouping, separate Deversoir/Kabrit identities despite a shared printed
token, grid-only distance sets, rejection of invented locations and byte replay.

Boxes are grounded in land:8.81 and airlog:34.81/43.1. Off-map facility names
and locators use scen:60.5 text plus the oob clerk's page transcription; this
is not an independent visual audit of the off-map facility table. No flight
range/connection, occupancy hex, capacity or original-sheet accuracy is claimed.
Printed E1833, E3433 and E4033 also exist as ordinary grid cells; the off-map
identities remain distinct from them. B5825/C4119/D3231/D3416/D3516/D3903 were
confirmed by the canonical lookup, with no facility-symbol verification claim.
National-region/Cairo memberships are still gaps, not accepted classifications.

## Graziani section D and Cairo (2026-10-06)

Current coverage: **227/7023 classified**, 6796 unknown. Counts are 191 clear,
22 rough, seven sea, two salt marsh and five major city. 228 distinct terrain
cells have decisions; C4026 remains deferred. Full-map accuracy is unknown.

`graziani-0004` reviews all 80 cells D first28..35/second08..17 at source scale:
68 clear, ten rough, two salt marsh (D3414, D3315). Against this single visual
pass, **1/49 non-abstaining proposals disagrees**: D3315 was proposed clear but
has the yellow-lined salt-marsh pattern. That is about 2.04% for this window's
non-abstaining predictions, not a full-map error estimate. Its publication is
manually corrected. The other 31 proposals abstained; all were visually resolved,
including salt marsh D3414. Contour splashes are separate from base terrain.
D3517 contains a small blue inland-water symbol: this was not assumed to be
Mediterranean coast; waterbody and directional hexside features remain unverified.

`cairo-0001` classifies the five building-group cells E1930/E1931/E1829/E1830/E1730
as major_city and records one place entry per cell, grouped under cairo. Four
contact sheets cover 54 distinct cells including the surrounding halo: E
first18..23/second30..34, first18..20/second27..32, first17/second27..32,
and first15..16/second28..33. Building groups inside
hex boundaries establish the five-cell city footprint; nearby labels, rail,
river and training symbols do not expand it into neighboring cells. Only five
city terrains are published from this inspection, not halo base classifications.
All five palette proposals abstained. The Cairo area now resolves to those five
canonical cells; Helwan E1430 remains distinct. Generic center circles in the
SVG are generated from place data, not traced building artwork.

These are single-observer checks, not an independent second review or 1979-map
verification. The failed D3315 proposal illustrates why palette agreement must
never stand in for visual checking. The section-C frontier symbol discrepancy
remains a gap; no national membership or border classification is claimed.

## Movement schema / initial masks (2026-10-07)

Source-bound terrain decisions regenerate227 terrain masks and228 coastal-domain
masks; C4026 remains unreadable terrain despite known coastal status. No edge
coverage or feature records are published. Per-kind unknown-versus-absent queries,
canonical seam queries, adjacency, directional high-side data and duplicate-mask
rejection are tested. Synthetic tests do not verify any map feature or classifier.
Approved Graziani window2933 cells/8547 internal edges/175 crossing edges includes
Tobruk, Bardia, Sollum, Matruh and both Alexandria city ids. This geometry-only
window is not a national polygon or playable-boundary decision. Random per-layer
classifier audits have not yet run; edge error rates remain unmeasured.

## Benghazi city membership (2026-10-07)

The native contact sheet contains40 actual source-contained cells from A
first45..51/second24..31. All were inspected for neighboring city groups, then
a city overview and shared-side enlargement checked the A4827/A4728/A4828
boundaries. One city building-group symbol is anchored in A4827; no distinct
city groups occur in the surrounding cells. El Berca village A4728 and Benina
airfield A4829 stay separate. This is visual source-profile evidence beyond the
garrison-id anchor, not inferred membership from scen-0001 alone. Building
artwork touches the southeast boundary; that overprint was checked and does
not supply a second city-symbol group in A4728. Port/training overprints do
not enlarge the city. Original1979map and independent-observer checks remain
unavailable. Benghazi area resolves to A4827 with review_batch=benghazi-0001.

A4827 is major_city terrain on coastal land. Its palette proposal abstained.
Only that city's terrain/place/area is accepted, not neighboring substrates or
port/facility attributes. Current coverage228classified/7023,6795unknown:
191clear,22rough,7sea,2salt_marsh,6major_city. Surface masks228terrain/229coastal;
edge masks are still empty. Total229distinct terrain cells have decisions.


## First movement-line pilot (2026-10-07)

The shipped detector is `line-pattern-v0.4`. Its tests caught a seed-sampling
failure: sampling only the bright gap could skip both road strokes. The fix
samples across the strokes too. It also rejects fully black grid ink from gray
line matching, searches finer crossing orientations and abstains on clipped
image regions. Synthetic tests establish these safeguards, not map accuracy.

The initial Bardia/Sollum calibration strip C first39..45/second20..25 contains
26 cells and 88 touching edges. An earlier v0.2 stratified sample disagreed on
0/12 resolved road predictions, 5/16 track predictions and 5/15 railroad
predictions. One further railroad crossing was unresolved. Grid lines, labels
and escarpments caused gray-line false positives; these calibration observations
were used to revise the detector and are not held-out validation. Their source
sheets and numeric audit remain local in scratch/runs/lines/pilot-0003.

After freezing v0.4, a separate adjacent strip C first44..48/second17..22 supplied
23 contained cells and 70 touching edges. Seed6031 selects up to eight rows per
kind and proposal state (all rows when a stratum is smaller). All 56 selected
kind/edge rows were checked visually against native sheets with the shared side
marked. One boundary-coincident track at C4320/C4419 stays unresolved. This is a
single-observer check on the 2021 map, not independent review or original1979
verification. Adjacent strips share some boundary cells/edges; this is a separate
strip check, not a statistically independent geographic holdout.

| Layer | Resolved positive predictions: errors/sample | Resolved negative predictions: errors/sample | Population abstentions |
|---|---:|---:|---:|
| Road | 0/5 | 0/8 | 3/70 |
| Track | 2/6 | 0/8 | 34/70 |
| Railroad | no positive examples | 0/8 | 2/70 |
| Unfinished road | no predictions | no predictions | 70/70 |

The sampled resolved prediction error is 0/13 for roads and 2/14 (14.29%) for
tracks. These strata have different sampling fractions, so the pooled numbers
are conditional sample descriptions, not population/full-map estimates.
Railroad positive accuracy is unknown: no positive example was present in this
strip, and the failed calibration prohibits relying on gray width alone.
Unfinished-road accuracy is unknown because this model always abstains. The
unfinished railroad/pipeline layers have no predictions or measured error rate.
Uncertain samples are not treated as errors or successes: three road cases,
ten railroad cases including two abstentions, and eight unfinished-road cases
were resolved absent; seven track abstentions were resolved absent, one stays
unresolved. Two false track positives (C4318/C4418, C4418/C4518) were corrected
to absence before publication.

`line-reviews/validation-0001` preserves every proposal and audited label with
source identities, exact seed, populations, file hashes, notes and citations.
The accepted subset supplies 55 per-kind edge masks (road16, track21,
railroad10, unfinished_road8) and nine positive rows (road5, track4). The
C4320/C4419 road is verified while its track layer remains unknown. No unsampled
prediction creates coverage. All hexside masks remain empty.

Inference took 4.469 seconds for70 edges on this machine; the calibration strip
took8.708 seconds for88. At those observed rates, the approved8722-edge window
would take approximately9?15 minutes for proposals only. This does not estimate
visual review time: track abstentions and errors remain substantial, unfinished
states need symbol-specific work, and no escarpment classifier has been audited.
Full-window line and hexside ETAs therefore remain provisional. Next steps are
reviewed strip expansion with connected route tracing, tighter dash/tie filters,
then an independent escarpment/high-side pilot. No traced artwork is published.


## Malta symbolic setup region (2026-10-07)

Source text scen:60.46 identifies a Malta air setup and an aggregate initial
facility capacity. airlog:44.11 identifies a Malta inset with different scale
and placement from the African map; airlog:34.81 includes Malta among possible
Commonwealth reinforcement destinations. These establish the symbolic region
`offmap_malta` and resolved area `malta`, with no African-grid hex membership.
The location has kind `box` and a source-named Malta Box locator. This is a
rules-text check, not a visual enumeration of the inset's individual fields.
No coordinate, port anchor, distance or per-field capacity is inferred. The
scenario owner retains the aggregate setup value in scenario data. Specific
Maltese airfield/mission targets remain unverified. Area generation now produces
33 selectors and 14 symbolic locations; terrain and feature coverage are unchanged.


## GT1-6 corridor and next terrain strip (2026-10-07)

The proposed source-contained, alias-resolved corridor has 834 cells and
2,541 touching edges (2,326 internal, 215 crossing). Before this batch,
176 corridor cells were classified and 658 unknown. Its manifest is a work
priority, not a legal scenario boundary or a surveyed layer mask.

`corridor-d-0001` inspects every one of the 75 source-contained cells in
D first34..43/second01..10 at native scale; four shoreline boundary crops
were enlarged. Six cells already in graziani-0004 agree with that review.
The 69 new decisions are 51 clear, eight rough and ten all-sea. Twelve new
cells are coastal. D3910 contains a small rough land fragment; D4004 has
cream land with contour splashes. Neither is classified from majority water.
Of the new proposals, 47 nonabstaining labels agree with visual review and
22 abstentions are resolved. These are conditional prediction disagreements,
not a random-population or independently reviewed error bound. All cells
were inspected by the same observer on the 2021 profile. Hexside and line
masks do not expand with this surface batch.

Retrospective proposal-output to local-commit intervals were 4m43s for the
70-cell inland batch (about 890 cells/hour), and 6m33s for the 80-cell D batch
plus Cairo work (at most 733 D cells/hour). These combine visual review,
record writing and tooling; no review-only timer or category split was kept.
A conservative planning allowance is 250-400 individually reviewed
cells/hour, with additional publication/check time. Classifier runtime is
not review throughput. The existing terrain and line samples do not justify
accepting unsampled predictions. A future sampled release must record its
sampling frame, seed, class-specific errors and confidence bounds, and
inspect abstentions and mixed-coast cells individually.


## First directly reviewed movement segment (2026-10-07)

`edge-reviews/road-spine-0001.toml` records 28 observations on two sides:
C4120/C4220 and C4020/C4120. Two roads are present, 24 other movement-kind
observations are absent, and two pipeline observations remain unresolved.
Thus it adds 26 per-kind masks: ten line-kind and sixteen side-kind masks.
Source-native overview, whole shared-side crops and surrounding-cell detail
were compared with the actual `vassal/extracted/images/TEC.png` key. The road
crosses both sides away from the corner. A preliminary C4121 descent was
rejected; the road descends through C4120. The nearby gray escarpment is on
other sides; the tied railroad by Fort Capuzzo lies southwest of these
crossings. The ordinary grid does not establish a printed border feature.
The full sides are land; all_sea absence is checked directly.

Every emitted observation was visually reviewed, with no classifier labels
accepted. No independent observer or random error bound is claimed; true
post-review error is unmeasured. Source-profile limitations remain. Three
route cell terrains were already individually accepted in graziani-0001.
The generated `strips.toml` names those cells and only the two completed
route edges. Both five-kind line and eight-kind side movement queries now
resolve on these pairs. Pipeline, adjoining sides and the enemy-control halo
remain unknown; supply, stacking, unit capability and actual action legality
are not certified by this map review.

Tests exercise unresolved evidence, source/key hash mismatch, canonical
adjacency, duplicate/overlapping reviews, positive direction/high side,
missing citations, preservation of old masks/features and old negatives,
and refusal to emit completeness with any route kind unknown. The public
SVG is new regular-hex art generated from canonical IDs and endpoints,
never a source-image tracing or recoloring.


## Sollum control review (2026-10-07)

The board's actual dev-profile movement C4020 to C3921 identified the next
review location; it was not used as evidence of map features. Eleven full
shared-side/surrounding-cell native crops around C4020 and C3921 were
viewed at 3x beside TEC.png. `edge-reviews/sollum-control-0001.toml` records
71 explicit kind-edge observations: 64 resolved (two tracks, three
escarpments, 59 negatives) and seven unresolved. Existing published masks
are not rewritten.
The 64 new masks comprise fourteen line-kind and fifty side-kind
observations. Total edge coverage becomes 145 masks, with 13 lines and
three positive side features. Terrain remains 297 classified cells.

The complete vertical C3921/C3922 shared side has the key's gray escarpment
band; its downslope splashes face C3922, making C3921 the high endpoint.
The coast road and dashed track inside C3922 do not cross this vertical
side. The neighboring blue line meets a different side at the vertex;
a vertex alone does not establish a crossing. Exact projected endpoint
locators and unmarked 4x crops also
resolve C3921/C4021 (C3921 high) and C4020/C4121 (C4020 high). A preliminary
negative on the former and abstention on the latter were corrected in the
unpublished draft; the blue symbol is on the adjoining side. The first two
contours form the fully surveyed branch C3922-C3921-C4021; all thirteen
movement kinds resolve on both sides. Canonical directions are E and NE;
reversing a query retains each source high endpoint. C4020/C4121 also has
a directly verified crossing track, but other line kinds remain unknown.
The branch SVG shows labeled regular-hex side bars, not copied contours.

C3921/C4020 has a separate thin dashed track crossing. The adjacent heavy
gray tied stroke and blue boundary-coincident blocks are not identified
confidently enough to accept railroad state or border presence/absence.
Both rail kinds and border remain unknown. Pipeline is unsurveyed on the
three surveyed route pairs;
C4020/C4021 escarpment identity/orientation remains unresolved. Earlier
informal rail-family descriptions
are not accepted classification of these exact sides.

Every published observation is directly reviewed, with no accepted
classifier output or second-observer claim. Post-review population error
and original1979equivalence remain unmeasured. Immediate water/river and
escarpment blocker queries around C3921 resolve, but other approach costs
and the larger control halo remain partial. Tests pin the real escarpment
high side, preserve the observed leg's unknown kinds, and do not convert
blocker-only halo coverage into full movement coverage.


## Bardia northern road review (2026-10-07)

Ten full-cell reviews in bardia-north-0001 add five clear, one rough, one
major_city and three sea classifications. Bardia buildings and the circled
port anchor are observed inside C4321; adjacent cells were checked for
buildings and shoreline fragments. C4420 is coastal clear and C4520 coastal
rough. C4421 and C4521 are entirely water, despite land in adjacent cells.
Brown edge bands are separated from the white substrate in C4320.

Exact endpoint locators beside unmarked native 4x source establish two
finished-road crossings: C4220/C4320 and C4320/C4321. The batch
bardia-road-0002 records 28 observations, 22 resolved and six abstentions.
Both slope and ridge remain unknown on each crossing: brown contour bands
and neighboring vertex joins need a firmer type and orientation reading.
Pipeline is unsurveyed. Other four movement line kinds and six side kinds
are reviewed absent on both crossings. No new complete strip is emitted.
All observations were directly reviewed by one observer; post-review
error rate, independent agreement and original1979 equivalence remain
unmeasured. Source images and locator crops stay outside the repository.


## Cartographer retained corridor surface batch (2026-10-07)

See review-notes/map-cartographer-0001.md and its source-bound raw batch:
122 cells individually inspected,120 new classifications105clear8rough7sea,
six coastal flags. The prior C4026 abstention is retained. No new edge
masks, facilities or completed movement strips are inferred. Single-observer
post-review error and original1979 equivalence remain unmeasured.

## Provisional mixed-land audit (map-0003, 2026-10-07)

Direct review of40 rough and2 salt-marsh cells across the earlier public
map and current local candidate identified seven earlier published
decisions that change under the provisional predominance rule: C2822,
C2922,C3024,C3124,D2917,D3414 are deferred; D3116 changes from rough to
clear with minor rough retained. Thirteen earlier mixed-land observations
receive explicit amendments, including retained classes and their minor
clear substrate. Candidate C3832 also defers; five other candidate mixed
rough/clear cells retain rough with minor clear recorded. Color-area
diagnostics guided this direct audit and never supplied accepted labels.
This is a bounded audit of non-clear classes, not a fresh independent
review of every earlier clear cell or a measured population error rate.


## Two verified port anchors (ports-0001,2026-10-07)

Three regional3x and full-cell6x native source views, their separate
registered locator twins, TEC.png and55.3 directly inspected. Two
accepted port points: Benghazi A4827 and Mersa Matruh D3814. Tobruk
shared-side overlap is withheld. No new terrain or edge review, city
extent or numeric port attribute supplied. Single-observer2021source;
population error and original1979 equivalence remain unmeasured.

## Alexandria positive city and port review

Batch alexandria-positive-0001 directly inspects E3613/E3714 as coastal major-city positives and the port wholly within E3613. Whole-cell6x/regional2.5x views, TEC.png, scenario60.41/60.5 and chart55.3 support these identities. E3713 spillover remains unresolved; no closed extent or place_group is published. Source views stay outside the repo; one observer, no independent error estimate or1979comparison. Every other terrain, edge and place remains unchanged.


Named-anchors-0001: six point markers were inspected on full native regional
views and separate registered locator twins, then compared directly to TEC.png
and scen:60.31. All six dots lie inside the cited canonical hexes. Only the
shared Village/Bir symbol family was established; no separate water subtype,
closed extent, fortified area or garrison permission is asserted. No source
terrain or coastal classification was added by these place records. Single
observer on the 2021 re-render; independent error rate and 1979 equivalence are
unmeasured. Source pixels remain local outside the repository.


## Map-team direct-review batches (2026-10-07)

The corridor uses direct review of every accepted full cell or physical shared
side on the native 2021 module image against TEC.png (land:8.37). Inspection
crops and registered locator twins remain outside the repository. No classifier
supplies accepted labels. Each immutable raw batch records its source hashes,
observer, exact selection, citations and abstentions; explicit amendments retain
older decisions. Generated outputs are replayed from the union of those inputs.

Map-terrain's current 516-cell assignment excludes the two Alexandria cells
transferred to cartographer. Its 460 initially unknown cells have exactly one
original review each: 433 classified and 27 deferred. Eight first-batch records
also have an explicit later amendment retaining their classes. Earlier parent
D2917/D3414 deferrals remain unknown, so the completed source scope projects
487 known and 29 unknown cells within this assignment. These counts distinguish
source scope from publication: the raw files and the current coverage masks
establish what is actually available in a given checkout.

The three assembler snapshots add the following evidence to the cd4c187 base:

| Snapshot | Terrain cells reviewed | Classified | Deferred | Edge pairs reviewed | Resolved edge-kind observations | Unresolved edge-kind observations |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| map-terrain-0006/0007 + map-lines-0005 | 100 | 95 | 5 | 60 | 773 | 67 |
| map-terrain-0008 + map-lines-2-0003 | 94 | 91 | 3 | 54 | 667 | 89 |
| map-terrain-0009 + map-lines-2-0004 | 91 | 81 | 10 | 68 | 844 | 108 |

Every unresolved edge-kind observation supplies no mask. These edge reviews use
the existing six line and eight side kinds. No lake presence or absence is
established, and old thirteen-kind movement completeness does not imply future
lake completeness. Route, control-halo and unit-action legality require their
own evidence and consumers.

Map-0003 is adopted by the owner: the clearly predominant LAND substrate decides
mixed-land cells, while unclear predominance remains unknown. Minor substrates
stay explicit in the evidence. Map-0002 still governs mixed land and marine sea.
E3413/E3414/E3514/E3614 have visible land but unresolved water identity; their
empty flags and notes retain unknown coastal status, with neither terrain nor
coastal-domain masks. The E3713 city-drawing spillover remains deferred.

Review throughput is an elapsed-time measurement, not an accuracy estimate.
The terrain worker inspected 462 original cells (including the two transferred
Alexandria cells) in about 116 minutes, approximately 239 cells/hour. A western
60-pair batch measured 288.4 source-reviewed pairs/hour and 55.9 published
pairs/hour including queue and checks; another bounded 60-pair batch measured
345.05 source-reviewed pairs/hour. These mostly clear batches do not establish
population throughput. Single-assembler publication rates are measured only
from actual verified main publication, separately for cells and physical pairs.

Per-layer post-review error rates and independent population agreement remain
unmeasured. Parent audits prove exact assignment coverage, canonical identities,
source provenance, immutable raw bytes and preservation of prior records; they
do not independently validate every worker source decision. Bounded second looks
are restricted to the examples explicitly recorded. No original-1979 map audit
or equivalence claim is supplied by these 2021 reviews.

### Three-worker edge cycle, 2026-10-07

Source batches `map-lines-0006`, `map-lines-2-0005` and
`map-terrain-edges-0001` directly review 192 previously unobserved physical
pairs, each against the fourteen currently supported kinds. Their 2,688
observations contain 57 present, 2,313 absent and 318 unresolved decisions.
They add 2,370 per-kind masks, twenty transport lines and 37 marine sides;
unresolved decisions add no masks. No terrain, places, lake observations or
complete route/control-halo certification is added. The union contains 7,938
edge-kind masks, 138 lines and 157 side features. Corridor terrain remains
801 known and 33 unknown.

All three workers retain source-local native inspection evidence and their
initial full-check receipts. The assembler preserves commit authorship and
raw decisions, checks disjoint fresh-pair membership and prior records, and
regenerates the union. The inland draft's Git line-ending normalization changes
CRLF to LF only; normalized byte equality retains its source decisions and
citations. The parent's TEC/source comparison for this first inland batch is
bounded to sixteen sides in two inspection sheets, not all forty sides.
Independent source accuracy remains unmeasured. Lake is still unsurveyed;
historical thirteen-kind movement completeness does not certify that layer.

### Second three-worker edge assembly, 2026-10-07

The checked `map-lines-2-0006`, `map-lines-0007` and `map-terrain-edges-0002` inputs add 225 distinct physical pairs and 3,150 source observations: 75 present, 2,787 absent and 288 unresolved. Their resolved observations add 2,862 per-kind masks, 22 line features and 53 side features (35 marine sides, 14 oriented slopes and four oriented escarpments). Pipeline remains unsurveyed on all 225 pairs; unresolved exact rail, river, marine endpoint, contour orientation, road and track identities receive no masks. No lake survey, new strip, complete control halo or unit-action legality follows from this assembly.

Each worker inspected its own whole sides against the native 2021 source and TEC and passed its initial map, Rust and web gates. The assembler preserves raw files, notes and original authorship, regenerates generated conflicts from the raw union, checks disjoint ownership and skips every prior physical pair including abstentions and pilots. Final joint gates and remote publication are recorded separately from source review. These checks establish transport, replay and inclusion, not an independent population source-error estimate or equivalence to the original 1979 sheets.

The resulting raw union has 856 classified terrain cells, 1,743 cell masks, 10,800 edge-kind masks, 160 line features and 210 side features. The corridor has observations of at least one kind on 868 of its 2,541 physical pairs; 1,673 pairs remain entirely unobserved. Counts of any observation are not completeness certificates. The 33 explicit corridor terrain gaps remain unchanged. No SVG file changed in the inland input; no reason for that unchanged output is asserted from the delta alone.
