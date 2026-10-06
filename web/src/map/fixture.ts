import hexCsv from '../../../data/map/hexes.csv?raw'
import aliasCsv from '../../../data/map/aliases.csv?raw'
/** Synthetic geometry only. Map ingestion will use data/map's owned schema when published. */
export type Terrain =
  | 'sea'
  | 'clear'
  | 'rough'
  | 'mountain'
  | 'salt_marsh'
  | 'sand'
  | 'unclassified'
export interface Hex {
  id: string
  q: number
  r: number
  terrain: Terrain
  label?: string
  src?: string
  flags?: string
}
export const TERRAIN: Record<Terrain, { color: number; label: string }> = {
  unclassified: { color: 0x52636a, label: 'Unclassified' },
  sea: { color: 0x193d4e, label: 'Sea' },
  clear: { color: 0xc0a875, label: 'Clear' },
  rough: { color: 0x957e57, label: 'Rough' },
  mountain: { color: 0x716c5b, label: 'Mountain' },
  salt_marsh: { color: 0x7e9b8c, label: 'Salt marsh' },
  sand: { color: 0xd3bc86, label: 'Sand' },
}
export const HEX_SIZE = 26
export function center(hex: Pick<Hex, 'q' | 'r'>) {
  return {
    x: HEX_SIZE * Math.sqrt(3) * (hex.q + hex.r / 2),
    y: HEX_SIZE * 1.5 * hex.r,
  }
}
export function vertices(hex: Pick<Hex, 'q' | 'r'>): number[] {
  const p = center(hex)
  return Array.from({ length: 6 }, (_, i) => {
    const a = ((i * 60 - 30) * Math.PI) / 180
    return [p.x + HEX_SIZE * Math.cos(a), p.y + HEX_SIZE * Math.sin(a)]
  }).flat()
}
export function syntheticMap(width = 100, height = 100): Hex[] {
  return Array.from({ length: width * height }, (_, i) => {
    const q = i % width,
      r = Math.floor(i / width)
    const coast = 8 + Math.floor(3 * Math.sin(q / 9))
    const terrain: Terrain =
      r < coast
        ? 'sea'
        : r === coast
          ? 'salt_marsh'
          : (q * 17 + r * 11) % 61 < 4
            ? 'mountain'
            : (q + r * 3) % 17 < 3
              ? 'rough'
              : r > coast + 10
                ? 'sand'
                : 'clear'
    return {
      id: `demo-${q}-${r}`,
      q,
      r,
      terrain,
      ...(r === coast + 1 && q % 14 === 0
        ? { label: `Outpost ${q / 14 + 1}` }
        : {}),
    }
  })
}
/** Consume cartographer CSV verbatim. Unrecognized terrain remains explicitly unclassified. */
export function parseMap(csv: string): Hex[] {
  const lines = csv.trim().split(/\r?\n/),
    header = lines.shift()!.split(',')
  const index = (key: string) => header.indexOf(key)
  return lines.map((line) => {
    const row = line.split(',')
    const rawTerrain = row[index('terrain')],
      terrain = rawTerrain in TERRAIN ? (rawTerrain as Terrain) : 'unclassified'
    return {
      id: row[index('hex_id')],
      q: Number(row[index('q')]),
      r: Number(row[index('r')]),
      terrain,
      src: row[index('src')],
      flags: row[index('flags')],
    }
  })
}
export const SYNTHETIC =
  import.meta.env.DEV &&
  typeof location !== 'undefined' &&
  new URLSearchParams(location.search).get('map') === 'synthetic'
export const HEXES = SYNTHETIC ? syntheticMap() : parseMap(hexCsv)
export const MAP_LABEL = SYNTHETIC
  ? 'SYNTHETIC TERRAIN'
  : 'REAL GRID · TERRAIN UNCLASSIFIED'
export const HEX_BY_ID = new Map(HEXES.map((h) => [h.id, h]))
const HEX_BY_COORD = new Map(HEXES.map((h) => [`${h.q},${h.r}`, h]))
for (const row of aliasCsv.trim().split(/\r?\n/).slice(1)) {
  const [alias, target] = row.split(',')
  const hex = HEX_BY_ID.get(target)
  if (hex) HEX_BY_ID.set(alias, hex)
}
/** Mock positions use real grid ids when the real grid is displayed, never invented printed ids. */
export function demoHex(q: number, r: number): string {
  return SYNTHETIC
    ? `demo-${q}-${r}`
    : (HEX_BY_COORD.get(`${q + 50},${r + 8}`) ?? HEXES[0]).id
}
export const INITIAL_HEX = demoHex(16, 12)
export function hexAt(x: number, y: number): Hex | undefined {
  const rf = y / (HEX_SIZE * 1.5),
    qf = x / (HEX_SIZE * Math.sqrt(3)) - rf / 2
  let q = Math.round(qf),
    r = Math.round(rf),
    s = Math.round(-qf - rf)
  const dq = Math.abs(q - qf),
    dr = Math.abs(r - rf),
    ds = Math.abs(s + qf + rf)
  if (dq > dr && dq > ds) q = -r - s
  else if (dr > ds) r = -q - s
  else s = -q - r
  return HEX_BY_COORD.get(`${q},${r}`)
}
