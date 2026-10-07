import data from '../data/rules.json'
import {
  CORRIDOR_COUNTS,
  TERRAIN_COUNTS,
  terrainCoverage,
} from './terrainCoverage'
import { HEX_BY_ID, TERRAIN } from './fixture'
export function TerrainCoverageControls({
  enabled,
  onChange,
  onHex,
}: {
  enabled: boolean
  onChange: (v: boolean) => void
  onHex: (id: string) => void
}) {
  const strips = data.map_strips.strips ?? []
  return (
    <section className="terrain-coverage">
      <label>
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => onChange(e.target.checked)}
        />
        Terrain classification &amp; corridor
      </label>
      <small>
        Classified {TERRAIN_COUNTS.known}; unclassified {TERRAIN_COUNTS.unknown}
        . Corridor {CORRIDOR_COUNTS.known}/{CORRIDOR_COUNTS.total} classified.
      </small>
      {enabled && (
        <p className="muted">
          Mint = classified; hatch = unclassified; blue outline = digitization
          corridor. Corridor membership does not certify routes or permit
          movement.
        </p>
      )}
      {strips.map((strip) => (
        <div className="reviewed-strip" key={strip.id}>
          <button onClick={() => onHex(strip.route_hex_ids[0])}>
            Locate {strip.id}
          </button>
          <small>
            Reviewed route: {strip.route_hex_ids.join(' / ')}.{' '}
            {strip.complete_line_kinds.length} line and{' '}
            {strip.complete_side_kinds.length} side kinds complete on these
            pairs only; control halo{' '}
            {strip.control_halo_complete ? 'surveyed' : 'unknown'}, pipeline{' '}
            {strip.pipeline_complete ? 'surveyed' : 'unknown'}. Action legality
            depends on the unit.
          </small>
        </div>
      ))}
    </section>
  )
}
export function TerrainCoverageInspector({ hexId }: { hexId: string }) {
  const hex = HEX_BY_ID.get(hexId)
  if (!hex) return null
  const c = terrainCoverage(hex)
  return (
    <p className="terrain-classification" data-testid="terrain-classification">
      {c.classified
        ? `Classified: ${TERRAIN[hex.terrain].label}`
        : 'Unclassified terrain'}
      {c.corridor ? ' / digitization corridor' : ' / outside corridor'}
    </p>
  )
}
