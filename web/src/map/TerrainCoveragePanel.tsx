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
      <button onClick={() => onHex('C4120')}>Locate Bardia coast</button>
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
