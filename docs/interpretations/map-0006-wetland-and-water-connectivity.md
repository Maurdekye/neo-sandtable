# map-0006 - Wetland binding and water connectivity in the 2021 render

- **Cases:** land:8.37, land:10.21
- **Status:** adopted
- **Profile version:** geometry `vassal-2021`, map schema 1; no terrain enum change
- **Decided by:** neo-sandtable, 2026-10-09, under the owner's delegated judgement
- **Owner review:** approved 2026-10-09 with the 29-cell corridor adjudication

## Question

Some wetlands in the supplied render differ from the terrain-key examples,
and nearby blue bodies cannot be assigned a marine domain by appearance.
Bind those land patterns and measure water connectivity separately.

## Evidence

Under land:8.37, delta has a flat green fill without strokes. The other
wetland patterns differ: swamp is the reed/grass wetland family, while the
render's salt marsh is beige with a yellow network, as in D2132, D2824
and D3414. The flat gray-green fringe east of Amiriya and Alexandria is
consistent with the irrigated delta. Geography supports the binding; it
does not replace the actual pattern or connectivity check.

This is an owner-delegated interpretation of the supplied 2021 VASSAL
render, not a newly discovered rule or an original 1979 equivalence claim.
Original source crops, raster masks and locators remain local-only.

## Ruling

- Flat gray-green fill is **delta**.
- Horizontal green lines with reed tufts are **swamp**. Cream gaps within
  the patterned body are its background, not separate clear terrain.
- Beige/yellow network remains **salt_marsh**.
- A cell still mixed between land classes uses amended map-0003: clear
  predominance first, otherwise the land substrate at its hex centre.
- Blue connected to the open Mediterranean, directly or through a port
  or harbour basin, is **sea**. Cells containing both that sea and land
  are coastal under map-0002; sea terrain still requires no land at all.
- Fully enclosed blue bodies are **lakes**, handled on shared sides under
  map-0004's midpoint-and-majority criterion. No lake mask follows merely
  from a cell containing water.
- Narrow blue bands crossing land are rivers or canals, represented as
  hexside features under the river rules, never as hex terrain. Their
  subtype, qualifying sides and coverage require direct separate review.

## Flood-fill method and seed

Use the full native raster, not an isolated cell crop. The source image
is `CNA Map Vassal Mitch Guthrie 2021.png`, SHA256
`904c884d0933e6dc21599243b038a4364d46a5f2151ac9bc8eda852d5c9b6011`.
The geometry build SHA256 is
`bd50ebff16fb2704fbfb28235a7e99acc5c6c5da6cb84bffcfa283659b9780ac`.

Coordinates are zero-based native image pixels with x right and y down.
The seed **(12200, 1600)** lies in unambiguous open Mediterranean water,
RGB **(138, 181, 207)**, north of the Alexandria land strip. Its local
source view is retained outside the repository.

The initial blue mask uses integer RGB channels and all five conditions:
`B-R >= 40`, `B-G >= 15`, `G-R >= 15`, `B >= 150`, `G >= 90`.
These retain the blue fill and shaded blue shore pixels while excluding
cream land, gray-green delta and green reeds. Flood-fill with eight-neighbour
connectivity from the seed. Keep the raw mask and component labels in
local evidence; the seed component has 22,489,743 pixels in this source.
The initial mask SHA256, as row-major uint8 0/1 bytes, is
`d9a8260005bad1744700b83cf136e024d1c4614ec70312ac24ba5fe731cbb0c6`.

The strict raw mask has no dilation, closing, invented bridge or
hand-painted path. Blue label ink alone is not a water body. Graphic
overlays can interrupt a component; a negative raw connection with an
unresolved graphic barrier does not establish physical lake enclosure.
Keep the strict mask and its result even when an overlay audit is used.

### Individually documented printed-ink bridges

Neo-sandtable approved a bounded bridge audit on 2026-10-09. Sea
connectivity may pass through dark road, rail, quay, city or port ink
drawn over water, but only at individually documented bridges. For each
bridge record its exact native pixel coordinates, the ink type crossed,
and a gap no wider than the measured three-native-pixel bound. It must
never cross cream, delta, swamp or any other land fill. Retain a native
source-view crop for each bridge locally; never commit these crops.

The global dark-ink sensitivity experiment is evidence only, not an
accepted mask or an automatic bridge list. A qualifying chain needs
strict-blue segments joined only by individually source-reviewed ink
bridges, all the way from the cell's water component to the open-sea
seed. Check the complete printed hex for contact. Each of E3413, E3414,
E3514 and E3614 requires its own documented chain; one cell's conclusion
does not certify a neighbouring cell or every water component it contains.

If such a chain cannot be documented, the water domain stays unresolved.
Its land class still follows the legend binding and amended hex-centre
rule, so the land class and movement-cost input are recorded independently
of coastal-domain knowledge. A disconnected raw fragment is not thereby
certified as an enclosed lake. Report each audited cell as sea-connected,
enclosed lake or unresolved, preserving the raw and bridge evidence.

## Affected behaviour and tests

The corridor review records the bound land classes, centre/minor evidence,
and independent water-component observations. This interpretation itself
does not add lake/river hexsides, backfill absence, change movement costs,
certify routes, or implement Land's consumer. Source-bound replay must
preserve prior reviews and reject wrong or missing amendment targets.
Map-0004's compatible lake tooling/consumer publication remains separate.
Population source accuracy and runtime loader equivalence are unmeasured.
