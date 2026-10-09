# map-0003 - Predominant substrate in mixed land terrain

- **Cases:** land:8.37
- **Status:** adopted
- **Profile version:** geometry `vassal-2021`, map schema1; no schema change
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** reviewed 2026-10-07 by the owner (batch review 3)

## Question

Some source cells contain substantial regions of more than one land terrain
class. Which single terrain value should the map publish? D3715, with clear
ground and a small salt-marsh region, prompted this question.

## Evidence

The TEC in land:8.37 distinguishes the land classes but the lead's search
found no controlling2021 case for assigning a mixed-land cell to one class.
The available source is the2021 VASSAL rendering, not an original1979 map.
This is a provisional interpretation, not a rule discovered in the chart.

## Ruling

Use the visibly predominant land substrate: the class occupying clearly the
largest land region inside the complete printed hex boundary. A minor edge
sliver does not determine the cell's class. If no class clearly predominates,
use the land substrate at the hex centre under the owner amendment below.
Keep unclassified when no land substrate is readable, or when the exact
palette-distance tie described below requires a further lead ruling.

Record each visible minor land substrate in the source-bound review evidence,
including unchanged decisions. Preserve earlier records with explicit batch
amendments so an owner overturn can be replayed mechanically. Decorative
contour strokes, shoreline edging, facility symbols and printed labels do not
constitute additional land substrates. Raster color counts may guide inspection
but do not decide the semantic footprint of patterned terrain.

Map-0002 is unchanged. Any visible land still makes a sea-mixed cell coastal;
predominance is assessed among its land substrates, excluding sea. An unreadable
small land fragment remains unclassified/coastal. Cities and facilities remain
subject to their existing separate evidence requirements.

## Rationale

Predominance gives the single terrain field a consistent meaning while keeping
minor regions available for an owner reversal. The owner amendment supplies
a centre tie-break when predominance cannot be established confidently.
This decision does not infer road, contour or hexside absence.

## Affected behaviour and tests

The source-locked review loader already supports accepted/deferred cells and
explicit batch-targeted amendments. The map-0003 audit uses that mechanism;
no runtime schema, terrain enum or numeric predominance threshold is added.
Existing amendment and mask tests exercise the replay and unknown policy.

A bounded direct audit of40 rough and2 salt-marsh cells found seven earlier
published classifications that change provisionally: C2822,C2922,C3024,C3124,
D2917,D3414 defer; D3116 becomes clear with minor rough retained. The unpublished
C3832 candidate also defers. Review amendments preserve previous decisions
and minor substrates. Earlier clear-cell minor-substrate audit remains
incomplete, so this is not a whole-map retrospective verification or measured
error-rate claim. Original1979 comparison remains unavailable.

## Owner ruling 2026-10-09 (question q0a0db88df999)

The owner amended the mixed-land decision through neo-sandtable on
2026-10-09. Status remains **adopted**. When no land class clearly
predominates within the printed hex, classify the land substrate beneath
the calibrated hex centre. If the centre lies on transport, label, contour
or symbol ink, use the nearest readable substrate pixel instead. Minor
substrates remain recorded. A patterned terrain's background between its
strokes belongs to that terrain, rather than being a separate clear patch.

This resolves the twenty mixed-land cells in section A of the local
29-cell adjudication sheet; that section label is not geographic Map A.
The same centre rule applies to future mixed-land reviews across the map.
No unrelated historical review is silently reclassified. Unknown remains
when no substrate is readable or an exact palette tie is unresolved.
Water identity remains a separate
question under map-0002 and map-0006; a centre in blue does not turn a
land-containing cell into sea terrain.

Nearest means native-pixel Euclidean distance from the calibrated centre.
Review evidence retains centre coordinates, whether it lands on graphic
ink, and the selected readable location. Equal-distance candidates with
different substrate identities are disclosed, not resolved by file order.

### Antialiased substrate boundary

Neo-sandtable approved this implementation convention on 2026-10-09 under
the owner's centre ruling. If the centre pixel is an antialiased blend
between substrate fills, rather than graphic ink, select the substrate
whose known palette colour has the smallest squared RGB distance from
the centre pixel. Candidates must be substrates actually present in its
immediate neighbourhood. Retain the other substrate as minor. Record the
native centre coordinate, its RGB value, the candidate palette colours
and both distances. Equal RGB distances remain Unknown and return to
neo-sandtable; no spatial or file-order tie-break is permitted.

D2516 at (9819, 2756), RGB (220, 214, 189), selects rough: squared RGB
distance 3117 to rough (194, 185, 149), versus 4757 to clear (251, 250, 239).
D2530 at (11013, 2756), RGB (216, 211, 185), selects rough: 2456 versus
5662 respectively. Clear remains minor in both. These are blended
substrate boundaries, not ink-covered centres; the spatial nearest-pixel
rule for graphic overlays does not decide them.

The twenty decisions and nine wetland/city decisions use one source-bound
review with an explicit prior-batch target per amended cell. Old reviews
and their minor-substrate evidence are preserved.
