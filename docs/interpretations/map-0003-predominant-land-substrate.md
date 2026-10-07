# map-0003 - Predominant substrate in mixed land terrain

- **Cases:** land:8.37
- **Status:** proposed (implemented provisionally)
- **Profile version:** geometry `vassal-2021`, map schema1; no schema change
- **Decided by:** neo-sandtable, 2026-10-07
- **Owner review:** pending, owner batch3

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
publish unclassified and leave its terrain layer outside the coverage mask.

Record each visible minor land substrate in the source-bound review evidence,
including unchanged decisions. Preserve earlier records with explicit batch
amendments so an owner overturn can be replayed mechanically. Decorative
contour strokes, shoreline edging, facility symbols and printed labels do not
constitute additional land substrates. Raster color counts may guide inspection
but do not decide the semantic footprint of patterned terrain.

Map-0002 is unchanged. Any visible land still makes a water-mixed cell coastal;
predominance is assessed among its land substrates, excluding sea. An unreadable
small land fragment remains unclassified/coastal. Cities and facilities remain
subject to their existing separate evidence requirements.

## Rationale

Predominance gives the single terrain field a consistent meaning while keeping
minor regions available for an owner reversal. Explicit abstention avoids a
numerical majority that cannot be established confidently from source art.
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
