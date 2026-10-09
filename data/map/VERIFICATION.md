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

### Third three-worker edge assembly, 2026-10-07

The checked `map-terrain-edges-0003`, `map-lines-2-0007` and `map-lines-0008` inputs cover 296 fresh physical pairs and 4,144 observations: 86 present, 3,585 absent and 473 unresolved. Their resolved decisions add 3,671 per-kind masks, 39 track crossings and 47 side features: 14 marine sides, 29 oriented slopes, two ridges and two oriented escarpments. Each worker directly inspected the native 2021 sides against the TEC and retained its initial map, Rust and web check receipts. Original raw records, notes, author names and dates are preserved through assembly.

The assembler verifies disjoint ownership, excludes prior physical pairs including abstentions and pilots, preserves earlier rows and replays the raw union twice byte-for-byte. These checks establish record preservation and reproducibility; independent source accuracy and equivalence to the 1979 sheets remain unmeasured. Final combined gates and actual remote publication are separate receipts. There are no lake observations or inferred lake negatives, new route or strip manifests, complete control halos or unit-action legality claims.

This union contains 856 classified terrain cells, 1,743 cell masks, 14,471 edge-kind masks, 199 line features and 257 side features. The corridor has at least one recorded observation on 1,164 of 2,541 pairs, leaving 1,377 entirely untouched. Geographic western/inland/eastern inventories are 440/969, 204/732 and 520/840 observed pairs. These counts do not certify movement completeness. All 33 corridor terrain gaps remain unchanged, including C3832 on four newly reviewed incident sides.

### Fourth three-worker edge assembly, 2026-10-07

The checked `map-lines-2-0008`, `map-lines-0009` and `map-terrain-edges-0004` inputs cover 300 fresh physical pairs and 4,200 observations: 96 present, 3,703 absent and 401 unresolved. The resolved decisions add 3,799 per-kind masks, 44 tracks and 52 side features: three marine sides, 24 oriented slopes, five ridges and twenty oriented escarpments. Every positive, source citation and required high endpoint matches its immutable raw record. High endpoints come from direct source-side observations, not endpoint terrain classes.

Each worker directly inspected its native 2021 sides against the TEC and passed initial map, Rust and web checks. Assembly preserves raw files, notes, author names and dates, prior rows and disjoint ownership; all earlier physical pairs, including unresolved records and pilots, are excluded from fresh selections. Two raw-union replays reproduce the same bytes. These checks do not supply an independent source-error estimate or establish 1979 equivalence. Actual final gates and remote publication have separate receipts.

The union contains 856 classified terrain cells, 1,743 cell masks, 18,270 edge-kind masks, 243 lines and 309 side features. Corridor terrain remains 801 known and 33 unknown. At least one observation exists on 1,464 of 2,541 corridor pairs, leaving 1,077 wholly unobserved; western/inland/eastern inventories are 540/969, 304/732 and 620/840 observed. Any-observation coverage is not a movement-completeness certificate. No lake survey, new route or strip, complete control halo or unit-action legality is added.


### Fifth assembled edge cycle, 2026-10-07

The immutable `map-terrain-edges-0005`, `map-lines-0010` and `map-lines-2-0009` inputs cover 300 fresh physical pairs and 4,200 observations: 74 present, 3,695 absent and 431 unresolved. The resolved decisions add 3,769 per-kind masks, 43 tracks and 31 side features: four oriented slopes, two ridges and 25 oriented escarpments. Every positive, source citation and required high endpoint matches its raw record. Directional highs come from direct source-side observations rather than endpoint terrain or coordinate direction.

The assembler preserves the original authors, dates and raw/note bytes. Old raw records, including abstentions and pilot observations, exclude repeated physical-pair review. Generated CSV conflicts are resolved by applying terrain and then regenerating layer outputs from the complete raw union. The 114-file map output is byte-identical across two replays. Full Rust, web, map and content checks remain required before main publication.

This union contains 856 classified terrain cells, 1,743 cell masks, 22,039 edge-kind masks, 286 lines and 340 side features. Corridor terrain remains 801 known and 33 unknown. At least one observation exists on 1,764 of 2,541 corridor pairs, leaving 777 wholly unobserved; western/inland/eastern inventories are 640/969, 404/732 and 720/840 observed. Any-observation coverage does not certify movement completeness. No lake survey, new route or strip, complete control halo or unit-action legality is added.

Each worker directly inspected its own native 2021 source sides against the TEC. These are single-observer records; independent whole-batch accuracy, population error and original 1979 equivalence remain unmeasured. Worker initial checks, assembler final checks and source accuracy are separate evidence. Review timing excludes different operations across workers and does not establish a comparable whole-corridor speed or accuracy estimate.


### Sixth assembled edge cycle, 2026-10-07

The immutable `map-lines-0011`, `map-lines-2-0010` and `map-terrain-edges-0006` inputs cover 260 fresh physical pairs and 3,640 observations: 79 present, 3,241 absent and 320 unresolved. Resolved decisions add 3,320 per-kind masks, 31 tracks, two unfinished roads and 46 side features: 30 marine sides, four oriented slopes and twelve oriented escarpments. All positives, source citations and required high endpoints match the raw records. Marine sides were inspected as whole segments with endpoints; neither marine status nor high direction is inferred from cell terrain.

Original authors, dates and raw/note bytes are preserved. Prior raw observations, pilots and abstentions exclude repeated physical-pair review. The 120-file map union is byte-identical across two terrain-then-layer replays. Full Rust, web, map and content checks remain required before publication. One worker's PowerShell wrapper attached errors to normal progress on stderr; actual child completion and terminal PASS evidence are distinguished from that wrapper exit, and the assembler runs its own required final gates.

The union contains 856 classified terrain cells, 1,743 cell masks, 25,359 edge-kind masks, 319 lines and 386 side features. Corridor terrain remains 801 known and 33 unknown. At least one observation exists on 2,024 of 2,541 corridor pairs, leaving 517 wholly untouched. Western/inland/eastern inventories are 740/969, 504/732 and 780/840 observed. These inventories certify observations only, not complete movement constraints. C3124 remains deferred on two reviewed western edges.

Evidence is direct native 2021 source review against the TEC by each worker. Whole-batch independent source accuracy, population error and original 1979 equivalence are unmeasured. No lake survey, new route or strip, complete control halo or action legality is added. The remaining terrain and uncertain coastal domains are unchanged.


### Seventh assembled edge cycle, 2026-10-07

The immutable `map-lines-0012`, `map-lines-2-0011` and `map-terrain-edges-0007` inputs cover 260 fresh physical pairs and 3,640 observations: 48 present, 3,213 absent and 379 unresolved. Resolved decisions add 3,261 per-kind masks, 25 tracks, four unfinished roads, one road and eighteen directly reviewed whole marine sides. Original authors, dates, raw/note bytes and earlier inputs are preserved. Generated tables, strip records and simplified SVG art replay mechanically from the raw union. The 127-file map union is byte-identical across two terrain-then-layer replays. Full Rust, web, map and content checks remain required before publication.

The eastern worker's current 840-pair share now has all fourteen supported kinds explicitly observed or unresolved: 11,760 records, 10,500 resolved masks and 1,260 abstentions without masks. This fulfills the current source-review inventory; it does not establish absence for deferred kinds. The four older authored D30 pairs transferred geographically retain their original evidence.

Seven directly source-observed track crossings form E2804â€“E2905â€“E2805â€“E2806â€“E2807â€“E2808â€“E2809â€“E2910. The eight route cells have known terrain and coastal masks, and each of the seven route edges has all thirteen current movement kinds resolved. The established strip generator emits regular hexagons and center links from structured data, without source art. This is current-schema map-layer completeness only: pipeline, future lake coverage, adjacent control halo and unit action legality remain unverified. The separate E2814/E2915 road is excluded because E2915 terrain is unknown.

The union contains 856 classified terrain cells, 1,743 cell masks, 28,620 edge-kind masks, 349 lines and 404 side features. Corridor terrain remains 801 known and 33 unknown. At least one observation exists on 2,284 of 2,541 corridor pairs, leaving 257 wholly untouched. Western/inland/eastern inventories are 840/969, 604/732 and 840/840 observed. These physical inventories include abstentions and do not establish full movement constraints. C3024/C3124 terrain and outside-corridor D2705â€“D2710 terrain/coastal gaps remain unchanged.

Each worker reviewed the native 2021 source against the TEC. Population error, independent whole-batch source agreement and original 1979 equivalence remain unmeasured. No lake survey, terrain/place change, complete control halo or action certification is added. The four uncertain coastal domains remain unknown.


### Final fresh corridor-edge inventory cycle, 2026-10-07

Four separate authored inputs (`map-lines-0013`, `map-lines-0014`, `map-terrain-edges-0008` and `map-terrain-edges-0009`) cover the final 257 wholly untouched corridor pairs. Their 3,598 explicit current fourteen-kind observations contain 41 present, 3,189 absent and 368 unresolved decisions. They add 3,230 masks and 41 source-observed tracks, with no new side or high-endpoint features. All unresolved decisions remain unmasked. Authors, dates, canonical raw/note bytes, earlier inputs and existing strips are preserved. The raw union mechanically regenerates the tables; full final Rust, web, map and content checks remain required before publication.

After this inclusion, each of the 2,541 corridor physical pairs has at least one observation: west 969, inland 732 and east 840. This physical inventory includes partial historic pilots and explicit abstentions. It is not fourteen-kind coverage or full movement certification. Thirteen western historic pilot pairs still lack some current-kind records; their exact layer gaps are listed below in GAPS.md. Earlier physical pairs were excluded from the fresh survey according to the partition ruling, so these records remain unchanged rather than being silently re-reviewed or treated as absence.

The inland worker's 728 initially fresh pairs have 10,192 current-kind records: 109 present, 8,909 absent and 1,174 unresolved, yielding 9,018 masks. Four previously published pairs retain their original evidence. The eastern current share remains 840 pairs with all fourteen kinds explicitly observed or unresolved, 10,500 masks and 1,260 unmasked abstentions. Western physical completion retains historical layer gaps and unresolved decisions; source review completion is distinct from resolved movement constraints.

The combined data contains 856 classified cells, 6,167 unclassified cells, 1,743 cell masks, 31,850 edge-kind masks, 390 lines and 404 side features. Corridor terrain remains 801 known and 33 unknown. No terrain, place, national selector, strip or schema change is introduced in this cycle. Outside-corridor unclassified endpoint surfaces also remain unknown. Source-observed tracks do not establish cell substrate or coastal status.

Reviews use the native 2021 source against the TEC. Source population error, independent whole-batch accuracy and original 1979 equivalence remain unmeasured. Pipeline abstentions, unsupported future lake coverage, adjacent control halos and unit action legality remain unknown or unverified. Previously validated strips certify their stated current movement layers only, with no additional whole-corridor legality claim.


### Combined terrain and historic-pilot gap review, 2026-10-08

The new eleven terrain amendment batches reinspect exactly the assigned 33
unknown corridor cells, grouped by their prior controlling batch. Four are
accepted and 29 remain deferred with fresh reasons. The earlier raw reviews
remain byte-identical. Four terrain masks are added; no coastal mask is added.
Existing coastal-mask citations for 29 assigned cells advance to the fresh
controlling review without changing their coverage identities or domains.
The four ambiguous water cells still have no terrain or coastal mask.

The assembler independently compared complete native 8x cell views and the
actual TEC for the four accepted cells only. These bounded checks agree with
the classifications. The 29 deferrals are the worker's direct observations;
no second-observer agreement or measured population error is claimed for them.
The review date is the worker's local 2026-10-08 date. Native 2021 review does
not establish equivalence to the original 1979 sheets.

The separate `map-lines-0015` partial survey contains 143 explicit decisions:
eight present, 110 absent and 25 unresolved. It adds 118 per-kind masks,
three line features and five sides. Mandatory slope high endpoints and all
citations match the raw records. Earlier inputs and feature rows are preserved.
The pilot-track deferral remains historical evidence alongside its fresh
direct absence; it is not counted as a second active Unknown.

All 2,541 corridor pairs now have explicit records for every supported kind:
35,574 slots comprise 31,923 resolved masks and 3,651 active abstentions.
The 2,541 legacy pipeline abstentions retain their bytes and are excluded
from future static survey requirements; 1,110 other kind identities remain
unresolved. This accounting supplies no missing lake evidence or action proof.

The combined union has 860 classified cells and 6,163 unclassified cells.
Corridor terrain is 805 known and 29 unknown. It has 1,747 cell masks,
31,968 edge-kind masks, 393 line features and 409 side features. Outside-scope
cells, places and existing strips remain unchanged. The 159 tracked map files
reproduce byte-identically across two terrain-then-layer replays. Worker
initial checks, assembler final gates and actual remote publication are
separate receipts; no source error rate is inferred from passing software tests.


### Libya-Egypt frontier and regions, 2026-10-08

One observer directly reviewed every one of 83 complete printed frontier sides
against the native 2021 map/key, plus the partial coastal C4122/C4221 side.
The lead independently confirmed that northern incidence only. An assembler
orientation audit detected a single swapped country label at C3819/C3920;
fresh native context inspection corrected it through a new supplemental
receipt, preserving the original source handoff. This is one detected metadata
error, not a measured population source error rate. No original 1979 map was
available for incidence comparison. Reduced complete A/B/D/E overviews supplied
no additional national border at their scales; exhaustive native absence and
whole-source agreement remain unmeasured.

The 84-wall fill separates C into 912 Libyan and 591 Egyptian canonical cells.
Whole country memberships are 3878 and 3078; map_c_libya has 912 and
map_c_or_d_egypt 1797. Sixty-seven verified Sea cells belong to neither; all
other 6956 cells partition with unknown terrain retained geometrically.
No unsourced coastal bridge or whole-map land mask is constructed. Software
checks cover missing walls, unknown coastal bypasses, incorrect orientation,
uncited/duplicate incidences, Sea coverage and seam aliases. Publication gates,
byte replay and compatible consumer checks have separate exact-commit receipts.
Old map inputs and movement feature/mask bytes remain unchanged.

### First Cyrenaica terrain wave, 2026-10-08

Three observers directly inspected disjoint sets of 75 complete cells each on
native 2021 views against the TEC. The immutable reviews are
`map-terrain-cyrenaica-0001`, `map-lines-cyrenaica-0001` and
`map-lines-2-cyrenaica-0001`. Their 225 observations contain 210 accepted
classes and 15 explicit deferrals: 154 clear, 31 rough, three gravel, one
major city and 21 marine Sea. Adopted map-0003 governs predominant land
substrates; minor substrates remain in individual review evidence. Contour
ink alone supplies no substrate class.

The assembler checked the exact allocations, disjointness, citations, prior
rows and raw/note blobs, and preserved all three original author identities
and dates. This is a data/provenance audit, not independent visual agreement.
Population source error and original 1979 equivalence remain unmeasured.
C4807 city terrain establishes no Tobruk port, closed city extent or capacity.

The union adds 432 cell masks: 210 terrain and 222 coastal-domain masks.
Three uncertain inland water identities have neither mask. Prior cell masks,
places, movement features and strips remain unchanged. There are now 1,070
classified cells, 5,953 unclassified cells and 2,179 cell masks; existing
31,968 edge-kind masks, 393 lines and 409 sides are preserved.

Replaying the existing area generator removes exactly the 21 newly verified
Sea cells from Libya and map_c_libya, with no additions or other region-field
changes. Their geometric counts become 3,857 and 891; known-land counts grow
from 36 to 225 and from 35 to 224 respectively. Egypt's regions are unchanged.
Unknown terrain stays in the geometric partition, with existing DEV/FULL
known-land consumer semantics. These counts supersede the preceding frontier
counts for this terrain union; they do not establish a complete setup domain.
All 166 map files reproduce byte-identically through two terrain, layer and
area replays. Initial worker Rust runs found obsolete live-count fixtures;
owner fixture correction, full combined gates and exact main publication are
separate receipts and must not be inferred from the map audit.


Northern Cyrenaica terrain continuation (132 directly reviewed cells): the
map-lines and map-terrain cyrenaica-0002 reviews add 120 classifications
(110 clear, nine rough, one Sea) and retain 12 explicit unknowns. All 119
newly classified land cells use the adopted predominant-substrate rule;
minor clear substrate in rough cells remains cited review evidence. This is
single-observer native 2021/TEC evidence, with no whole-batch second-observer
agreement, measured population error rate or 1979 equivalence claim.

The union adds 248 cell masks; the four unknown coastal domains C3908,
C3708, C3608 and C3508 add neither terrain nor coastal masks. Known-land
Libya grows from 225 to 344 and map_c_libya from 224 to 343. The generator
removes only newly verified Sea C4908 from these two derived regions; no
source frontier, geometry, other membership or place record changes. C4908
Sea terrain supplies no Tobruk port or other facility fact. All 170 map
files reproduce byte-identically through two terrain/layer/area replays.
Full joint gate results and exact publication remain separate receipts.


### Standing C-Libya terrain cycle 1, 2026-10-08

Three immutable worker reviews cover 193 previously unobserved cells:
map-terrain and map-lines cyrenaica-0003, and map-lines-2 cyrenaica-0002.
They add 188 land classifications (181 clear, five rough, two salt marsh)
and retain five explicit terrain Unknowns. Every accepted cell uses direct
native 2021/TEC review and the adopted predominant land-substrate rule
(land:8.37; interp:map-0003); mixed-cell minor substrates stay in the raw
notes. Authored raw and note bytes and author dates are preserved. This is
single-observer evidence; source error rates, independent batch agreement
and original 1979 equivalence remain unmeasured.

The union adds 381 masks: 188 terrain and 193 noncoastal-domain records.
Existing point flags, including C1715 Village/Bir, remain separate from
terrain observations. Known-land counts rise from 344 to 532 in Libya and
from 343 to 531 in map_c_libya. No Sea is added, so all generated area
memberships and fields remain unchanged. Prior raw records, geometry,
frontier, places, edges and strips are preserved. Whole-map totals become
1,378 classified cells, 5,645 unclassified cells and 2,808 cell masks;
31,968 edge-kind masks, 393 lines and 409 sides remain unchanged.

All 176 map files reproduce byte-identically through two complete terrain,
layer and area replays. Exact joint Rust/web gate results and main
publication are recorded separately; replay does not certify source
accuracy, a complete setup domain or new action permission.


### Standing C-Libya terrain cycle 2, 2026-10-08

Three immutable reviews cover 193 previously unobserved cells: map-terrain
and map-lines cyrenaica-0004, and map-lines-2 cyrenaica-0003. They add 168
land classifications (121 clear, eight rough, eight salt marsh and 31
desert), with 25 explicit terrain Unknowns. Direct native 2021/TEC review
uses the adopted predominant land-substrate rule (land:8.37;
interp:map-0003); minor substrates and abstention reasons remain in each
worker's raw notes. Authored raw/note bytes and author dates are preserved.
Source accuracy, independent observer agreement and 1979 equivalence
remain unmeasured.

The union adds 346 masks: 168 terrain and 178 coastal-domain records.
Fifteen inland water-identity deferrals have empty flags and supply no
terrain or coastal masks. The other ten terrain deferrals have reviewed
noncoastal domains and supply only domain masks. No Sea is accepted;
all area memberships and fields remain unchanged. Existing point flags,
prior raw records, geometry, frontier, places, edges and strips are
preserved. Known-land Libya grows from 532 to 700, and map_c_libya from
531 to 699. Whole-map totals become 1,546 classified cells, 5,477
unclassified cells and 3,154 cell masks; 31,968 edge-kind masks, 393 lines
and 409 sides remain unchanged.

All 182 map files reproduce byte-identically through two complete terrain,
layer and area replays. Full joint Rust/web gates and main publication
have separate receipts. Terrain evidence certifies no complete setup
domain, new movement side, place, facility or action permission.


### Standing C-Libya final terrain cycle, 2026-10-08

Two immutable cyrenaica-0005 reviews cover the final 134 previously
unobserved allocation cells. They add 125 land classifications: 34 clear,
nine rough, one salt marsh, 80 desert and one mountain, with nine explicit
Unknowns. Each worker directly reviewed full native 2021 cells against
the TEC under land:8.37 and interp:map-0003; minor substrates and
abstention reasons remain in raw notes. Raw/note bytes, original authors
and author dates are preserved. Parent checks verify provenance and
mechanical preservation; source accuracy, independent observer agreement
and 1979 equivalence remain unmeasured.

The union adds 259 masks: 125 terrain and 134 coastal-domain records.
All nine terrain deferrals supply only reviewed noncoastal-domain masks.
No Sea is accepted, so every area membership and field remains unchanged.
All prior observations, point flags, geometry, frontier, places, edges
and strips are preserved. Known-land Libya grows from 700 to 825, and
map_c_libya from 699 to 824. Whole-map totals become 1,671 classified
cells, 5,352 unclassified cells and 3,413 cell masks; 31,968 edge-kind
masks, 393 lines and 409 sides remain unchanged.

All 186 map files reproduce byte-identically through two complete terrain,
layer and area replays. Full joint gates and actual main publication have
separate receipts. Coverage of an allocation does not resolve its explicit
Unknowns, certify source accuracy or grant new action or facility permission.


### Tobruk/Bardia B-Libya terrain allocation, 2026-10-08

Three immutable reviews cover 110 previously unobserved canonical cells
in the existing Libyan B-section work allocation. Direct full native 2021
cell and boundary review against the TEC supplies 93 classifications:
75 clear, 12 gravel, three rough, one salt marsh and two Sea, with
17 explicit Unknowns. Each cell cites land:8.37 and adopted map-0002/
map-0003; observed minor substrates and abstention reasons stay in raw
notes. Original raw/note bytes, authors and author dates are preserved.
Parent audits establish provenance and mechanical preservation; source
accuracy, independent visual agreement and 1979 equivalence are unmeasured.

The union adds 191 masks: 93 terrain and 98 marine-only coastal-domain
observations. Twelve inland water-identity Unknowns have empty flags and
receive no masks; five substrate-balance Unknowns receive domain masks
only. B5133 and B5232 are directly reviewed Sea and remove themselves
only from generated Libya, with no additions or other area-field changes.
All prior raw observations, point flags, geometry, frontier, places, edges
and strips are preserved. Known-land Libya grows from 825 to 916;
map_c_libya stays at 824. Whole-map totals become 1,764 classified cells,
5,259 unclassified and 3,604 cell masks. Edge-kind masks, lines and sides
remain 31,968, 393 and 409.

All 192 map files reproduce byte-identically through two complete terrain,
layer and area replays. Full final joint gates and actual main publication
have separate receipts. Allocation coverage does not resolve explicit
Unknowns or grant a place, facility, movement-side or action permission.


### Derna/Mechili terrain, first snapshot, 2026-10-08

Three immutable reviews directly inspect 225 previously unobserved
canonical B-section cells in the existing Libyan work allocation. Every
full native 2021 cell and boundary was compared with the actual TEC.
The records supply 199 classifications: 73 clear, 46 gravel, 53 rough,
13 mountain, one salt marsh and 13 Sea; 26 remain explicitly Unknown.
Adopted map-0003 selects the clearly predominant land substrate, with
visible minor substrates and abstention reasons retained per cell.
Only directly established marine water supplies Sea/coastal evidence.
All records cite land:8.37 and adopted map-0002/map-0003; original
raw/note bytes, source pins, authors and author dates are preserved.
Parent audits establish provenance and mechanical preservation; visual
accuracy, independent observer agreement and 1979 equivalence are unmeasured.

The union adds 409 cell masks: 199 terrain and 210 marine-only
coastal-domain observations. Fifteen inland water-identity Unknowns
have empty flags and no masks. Eleven substrate-balance or obscured
land Unknowns have only domain masks; none has a terrain mask.
Thirteen directly reviewed Sea cells remove themselves only from
generated Libya, with no additions or other area-field changes.
All prior observations including Unknown, existing point/facility flags,
geometry, frontier, places, edges and strips are preserved. Existing
B5925/B4921 place anchors gain no new facility or extent facts.

Libya known land grows from 916 to 1,102; map_c_libya stays at 824.
Whole-map totals become 1,963 classified cells, 5,060 unclassified and
4,013 cell masks. Edge-kind masks, lines and sides remain 31,968,
393 and 409. All 198 map files reproduce byte-identically through
two complete terrain/layer/area replays. Required full final joint
gates, actual publication and independent inclusion have separate
receipts; source coverage never grants a complete setup/control domain,
place extent, movement-side fact or action permission.


### Derna/Mechili second terrain snapshot, 2026-10-08

Three disjoint source reviews cover exactly 225 further cells in the frozen
Derna/Mechili allocation. They supply 194 classifications: 104 clear,
43 gravel, 29 rough, nine mountain, two salt marsh and seven Sea;
31 remain explicitly Unknown. Each whole native2021 cell and boundary
was inspected against the actual TEC by its batch observer. Adopted
map-0003 selects the clearly predominant land substrate; observed minor
substrates and deferral reasons remain in the individual raw records.
Original raw/note bytes, source pins, authors and dates are retained.
Parent checks establish provenance and mechanical preservation; visual
accuracy, independent observer agreement and 1979 equivalence are unmeasured.

The union adds 398 cell masks: 194 terrain and 204 marine-only
coastal-domain observations. Twenty-one inland water-identity Unknowns
have empty flags and no masks. Ten land-substrate deferrals have only
domain masks; none has a terrain mask. Seven newly reviewed Sea cells
remove themselves only from generated Libya, without additions or other
area-field changes. Prior raw observations including Unknown, all prior
masks, point/facility flags, geometry, frontier, edges and places remain
preserved. These records add no facility anchor, extent or other-layer fact.

Libya known land grows from 1,102 to 1,289; map_c_libya remains at 824.
Whole-map totals become 2,157 classified cells, 4,866 unclassified and
4,411 cell masks. Edge-kind masks, lines and sides remain 31,968,
393 and 409. All 204 map files reproduce byte-identically in two complete
terrain/layer/area replays. Required final joint checks, publication and
independent inclusion have separate receipts. Coverage does not establish
a complete setup/control domain or grant action permission.


### Derna/Mechili final terrain snapshot, 2026-10-08

Three disjoint source reviews cover the final 191 cells of the frozen
641-cell Derna/Mechili allocation. They supply 174 classifications:
103 clear, 25 gravel, 29 rough, 13 mountain, two salt marsh and two Sea;
17 retain explicit Unknown terrain. Every full native2021 cell and boundary
was reviewed against the actual TEC by its batch observer. Adopted map-0003
chooses clearly predominant land substrates; minor substrates and honest
uncertainty remain in source-bound per-cell notes. Original raw/note bytes,
source pins, authors and dates are retained. Parent provenance and replay
checks do not measure visual accuracy or independent observer agreement;
1979 equivalence remains unmeasured.

The union adds 353 cell masks: 174 terrain and 179 marine-domain observations.
Twelve inland water-identity Unknowns have empty flags and no masks. Five
land-substrate Unknowns have only domain masks and no terrain masks. Newly
reviewed Sea B5731 and B5830 remove themselves only from generated Libya;
no additions or other area-field changes occur. Prior raw records including
Unknown, all prior masks, point/facility flags, geometry, frontier, edges
and places remain preserved. No lake, edge, port/place, facility anchor,
closed extent, setup/control completeness or action permission is added.

Candidate Libya known land grows from 1,289 to 1,461; map_c_libya remains
824. Whole-map totals become 2,331 classified cells, 4,692 unclassified and
4,764 cell masks. Edge-kind masks, lines and sides stay 31,968, 393 and 409.
All 210 map files replay byte-identically twice. Final joint gates,
publication and independent inclusion are recorded separately. The full
641-cell allocation is observed exactly once: 545 known land, 22 Sea and
74 explicit Unknowns. Allocation completion does not resolve those gaps
or establish source accuracy.


### Msus/Beda Fomm/Benghazi first terrain union, 2026-10-08

Three disjoint frozen 79-cell selections contain 237 directly reviewed cells.
Workers inspected native 2021 full interiors and printed boundaries against
the actual TEC, following land:8.37 and adopted map-0002/map-0003. The source,
build and key hashes, observed minor substrates, and individual reasons are
retained in the three unique raw reviews and matching review notes. This is
single-observer source evidence plus a parent provenance/preservation audit;
no independent visual agreement, measured error rate or 1979 equivalence
is established.

The union contains 204 accepted classes: 147 clear, 17 rough, 14 mountain,
6 salt marsh, 3 heavy vegetation and 17 Sea. It adds 187 known-land cells and
retains 33 explicit Unknowns. Of 419 new cell masks, 204 cover terrain and
215 cover directly established marine domains. The 22 empty-flag Unknowns
have no masks; the other 11 have domain masks only. All prior raw observations
including Unknown, point/facility flags, geometry, masks, places and features
are preserved.

Only the 17 newly accepted Sea cells remove themselves from generated Libya:
A3328, A3626, A3726, A4125, A4625, A4726, A4826, A4927, A5027, A5028, A5128, A5129, A5431, B5702, B5802, B5803, B5904.
No membership additions or other area fields change. All Unknowns remain
geometric members and requires_land is unchanged. The observations supply no
lake, edge, port/place, facility anchor, closed extent, complete setup domain
or action permission.

Candidate Libya known land grows from 1,461 to 1,648; map_c_libya remains 824.
Whole-map totals become 2,535 classified, 4,488 unclassified and 5,183 cell
masks. Edge-kind masks, lines and sides remain 31,968, 393 and 409. All 216
map files replay byte-identically twice. Final joint gates, publication and
independent inclusion are recorded separately. Later 472 source observations
stay outside this checked snapshot until preceding publication and inclusion.


### Msus/Beda Fomm/Benghazi second terrain union, 2026-10-08

Three disjoint frozen 79-cell selections add 237 directly reviewed cells.
Native 2021 full interiors and printed boundaries were inspected against
the actual TEC under land:8.37 and adopted map-0002/map-0003. Source/build/key
hashes, observed minor substrates and individual reasons are retained in
unique raw reviews and matching notes. Single-observer source evidence and
parent provenance/preservation checks establish no measured accuracy,
independent visual agreement or 1979 equivalence.

Accepted classes, counted from the immutable raw union: 160 clear, 5 gravel, 1 heavy vegetation, 1 mountain, 31 rough, 4 salt marsh, 14 sea.
These 216 classes add 202 known-land cells and 14 Sea cells; 21 observations
remain Unknown. The 438 new cell masks comprise 216 terrain and 222 domain
masks. Fifteen empty-flag Unknowns are entirely unmasked; six have domain
masks only. All prior observations, masks, point/facility flags, geometry,
places and features are preserved.

Only these newly accepted Sea cells remove themselves from generated Libya:
A3128, A3227, A3427, A3825, A3926, A4025, A4425, A4525, A5229, A5532, A5632, B5905, B6005, B6006.
No memberships are added and no other area fields or definitions change.
All Unknowns remain geometric members and requires_land is unchanged.
No lake, edge, port/place, facility anchor, closed extent, complete setup
domain or action permission is established.

Libya known land becomes 1,850; map_c_libya remains 824. Whole-map totals
become 2,751 classified, 4,272 unclassified and 5,621 cell masks. Edge-kind
masks, lines and sides remain 31,968, 393 and 409. All 222 map files replay
byte-identically twice. Final gates, publication and independent inclusion
are recorded separately. The remaining 235 source observations stay outside
this checked snapshot until preceding publication and inclusion.

The preceding first-union paragraph now correctly reports 14 mountain cells;
the earlier 13 was a prose tally error. Raw classes, generated data and
other first-union counts were unchanged.


### Msus/Beda Fomm/Benghazi final terrain union, 2026-10-08

Three disjoint frozen selections of 79, 78 and 78 cells add 235 directly
reviewed full printed cells. Actual native 2021 source and TEC are cited
under adopted map-0002/map-0003. Original author/date, source pins, explicit
minor substrates, Unknown reasons and all preceding inputs remain intact.
Single-observer source classifications and parent provenance/preservation
checks supply no measured source accuracy or 1979 equivalence.

Accepted classes counted from immutable raw: 166 clear, 4 gravel, 4 heavy vegetation, 3 mountain, 26 rough, 2 salt marsh, 7 sea.
These 212 accepted classes comprise 205 land and seven Sea cells; 23 remain
Unknown. Added masks total 435: 212 terrain and 223 domain. Twelve empty-flag
Unknowns are entirely unmasked and eleven have domain masks only. Prior
terrain/edge masks, geometry, point flags, places, features and strips are
preserved. Only these new Sea cells remove themselves from generated Libya:
A3527, A4225, A4325, A5330, A5331, A5633, B5701.
No additions or other area field changes occur; requires_land is unchanged.

Whole-map totals become 2,963 classified, 4,060 Unknown and 6,056 cell masks.
Libya known land becomes 2,055; map_c_libya remains 824. Edge masks, line
features and sides remain 31,968, 393 and 409. All 228 map files reproduce
byte-identically twice. These counts are source/data outcomes; final green
gates and actual publication are recorded separately. No lake, edge,
port/place, facility extent, complete setup domain or action permission
is established.

The western worker initial workspace matrix failed the existing movement
test wall-time guard. Its exact head, tree, raw exit and logs are preserved
separately; missing historical CPU/load measurements are unmeasured. Other
already-started worker matrices completed successfully. The lead identified
the wall-time guard as contention-sensitive and directed the consumer owner
to correct it separately. That failure is retained, not a map source/data
correction or a measured source error rate. Parent final publication still
requires the specifically authorized one complete green gate.

### Owner-adjudicated Graziani land classes, 2026-10-09

The single review `graziani-owner-20261009-0001` resolves exactly 29 prior
corridor land-class deferrals under the owner's amended map-0003 and delegated
map-0006 binding. Native full-cell views, centres, minor substrates and source
pins are retained locally. The two antialiased boundary centres use the
lead-approved squared-RGB comparison among immediately observed palettes:
D2516 rough 3,117 versus clear 4,757; D2530 rough 2,456 versus clear 5,662.
Both retain clear as minor. E3713 is the owner's additional Alexandria city
hex, retaining coastal identity and the same city name as its neighbours.
This does not establish a closed city extent or additional port capacity.

Classes are 15 rough, two clear, one mountain, three salt marsh, two delta,
five swamp and one major city. All prior review bytes are preserved. Whole-map
classified terrain increases from 3,039 to 3,068; 3,955 cells remain Unknown.
Coverage gains exactly 29 terrain masks. Existing edge-kind coverage, line
features, hexsides, aliases, sections and generated areas are unchanged.

E3413, E3414, E3514 and E3614 retain unresolved water domains and have no
coastal mask, despite known land classes swamp, swamp, clear and delta.
The raw eight-neighbour blue flood-fill from native seed (12200, 1600) remains
preserved. A regional search linked strict-blue components only through at
most three consecutive dark pixels and found no individually documented
chain for these four cells. The wider dark-ink sensitivity experiment is
diagnostic only. This negative bounded result is not a lake-enclosure proof;
no lake/river-side masks or inferred coastal absence are added.

Two source-bound stdlib replays reproduce every map file byte-identically.
Exactly these 29 hex rows change, and 75 map unit checks pass, including
wrong/missing prior-batch targets, duplicate rejection, independent terrain
and coastal coverage, the historical D3414 deferral and the owner city row.
Rust, web, native admission and publication remain separately sequenced
checks; these source checks do not claim them. Single-observer source evidence
and hash/replay checks supply no measured classification error rate or 1979
equivalence. The inherited parent rust-slow CI failures remain separate engine
findings and do not become a CI-green claim for this candidate.

### Graziani corridor lake source survey, 2026-10-09

The first pass inspected all 2,541 canonical adjacent physical pairs touching
the 834-cell corridor: 2,326 internal and 215 crossing pairs. Its immutable
historical buckets remain seven provisional candidates, 20 uncertainties
and 2,514 unmasked controls. A final, separate current-adjudication overlay
for the 27 candidate or uncertain pairs records zero confirmed lake sides,
13 marine not-lake sides and 14 Unknowns under adopted map-0004/map-0006.
Reinspection adds no unique pairs and rewrites no historical observation.

Native source review, exact identity and geometry reconstruction, predecessor
hash preservation and actual-source RGB/sample checks were independently
reviewed. Raw disconnected blue components alone do not prove enclosure;
no printed-ink bridge or domain enlargement was used. Classification error
rates and original 1979 equivalence remain unmeasured. All first-pass pairs
are accounted for, but the 14 Unknowns prevent an exhaustive supported lake
count. Their exact identities and current non-lake engine treatment are in
[GAPS.md](GAPS.md#graziani-corridor-lake-side-uncertainty-2026-10-09).

The lead closes this source scope with documentation only. No lake masks,
new SideKind, ZOC implementation or pipeline-completeness change is added.
Lake's potential effect is ZOC blocking (land:10.21a), with no Lake movement
cost in the land:8.37 TEC. Compatibility work is deferred until an applicable
play need, a confirmed lake or delegated water adjudication supplies a reason
to revisit it. This survey supplies no route, setup or action certification.
