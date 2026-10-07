import data from '../data/rules.json'
import { HEXES, HEX_BY_ID, SYNTHETIC, type Hex } from './fixture'
export const CORRIDOR = new Set<string>(
  SYNTHETIC ? [] : (data.map_corridor.hex_ids ?? []),
)
export function terrainCoverage(hex: Hex) {
  return {
    classified: hex.terrain !== 'unclassified',
    corridor: CORRIDOR.has(hex.id),
  }
}
export const TERRAIN_COUNTS = {
  known: HEXES.filter((h) => terrainCoverage(h).classified).length,
  unknown: HEXES.filter((h) => !terrainCoverage(h).classified).length,
  corridor: HEXES.filter((h) => terrainCoverage(h).corridor).length,
}
export const CORRIDOR_COUNTS = {
  known: [...CORRIDOR].filter(
    (id) => HEX_BY_ID.get(id)?.terrain !== 'unclassified' && HEX_BY_ID.has(id),
  ).length,
  total: CORRIDOR.size,
}
