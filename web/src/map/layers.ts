import lineCsv from '../../../data/map/line_features.csv?raw'
import sideCsv from '../../../data/map/hexsides.csv?raw'
import coverageCsv from '../../../data/map/coverage.csv?raw'
import data from '../data/rules.json'
import {
  center,
  HEXES,
  HEX_BY_ID,
  HEX_SIZE,
  SYNTHETIC,
  type Hex,
} from './fixture'
export type Layer = 'terrain' | 'coastal' | `line:${string}` | `side:${string}`
if (data.map_manifest.schema_version !== 1)
  throw new Error('Unsupported map layer schema')
export const FEATURE_LAYERS: Layer[] = [
  ...data.map_manifest.line_kinds.map((kind) => `line:${kind}` as Layer),
  ...data.map_manifest.hexside_kinds.map((kind) => `side:${kind}` as Layer),
]
export const COVERAGE_LAYERS: Layer[] = [
  ...(data.map_manifest.cell_layers as Layer[]),
  ...FEATURE_LAYERS,
]
export interface Edge {
  a: Hex
  b: Hex
  key: string
}
export interface Feature {
  layer: Layer
  edge: Edge
  highSide?: string
  src: string
}
export interface Layers {
  features: Feature[]
  coverage: Map<Layer, Set<string>>
}
export interface LayerOptions {
  visible: Layer[]
  coverage: Layer | null
}
export const DEFAULT_LAYERS: LayerOptions = {
  visible: FEATURE_LAYERS,
  coverage: 'line:road',
}
export function label(layer: Layer) {
  return layer.replace('line:', '').replace('side:', '').replaceAll('_', ' ')
}
export function edgeKey(a: string, b: string) {
  return [a, b].sort().join('|')
}
/** CSV supports quoted fields and escaped quotes, including review notes with commas. */
export function rows(csv: string): Record<string, string>[] {
  const records: string[][] = []
  let row: string[] = [],
    cell = '',
    quoted = false
  for (let i = 0; i < csv.length; i++) {
    const c = csv[i]
    if (c === '"') {
      if (quoted && csv[i + 1] === '"') {
        cell += '"'
        i++
      } else quoted = !quoted
    } else if (c === ',' && !quoted) {
      row.push(cell)
      cell = ''
    } else if (c === '\n' && !quoted) {
      row.push(cell.replace(/\r$/, ''))
      if (row.some(Boolean)) records.push(row)
      row = []
      cell = ''
    } else cell += c
  }
  if (cell || row.length) {
    row.push(cell.replace(/\r$/, ''))
    records.push(row)
  }
  if (quoted) throw new Error('Unclosed map CSV quote')
  const header = records.shift() ?? []
  return records.map((r) =>
    Object.fromEntries(header.map((key, i) => [key, r[i] ?? ''])),
  )
}
const offsets = [
  [1, 0],
  [0, 1],
  [-1, 1],
  [-1, 0],
  [0, -1],
  [1, -1],
]
export function edges(hexes: Hex[]): Edge[] {
  const coords = new Map(hexes.map((h) => [`${h.q},${h.r}`, h]))
  return hexes.flatMap((a) =>
    offsets.flatMap(([dq, dr]) => {
      const b = coords.get(`${a.q + dq},${a.r + dr}`)
      return b && a.id < b.id ? [{ a, b, key: edgeKey(a.id, b.id) }] : []
    }),
  )
}
export const EDGES = edges(HEXES)
function resolveEdge(
  aId: string,
  bId: string,
  membership: Map<string, Hex>,
): Edge {
  const a = membership.get(aId),
    b = membership.get(bId)
  if (
    !a ||
    !b ||
    a.id === b.id ||
    !offsets.some(([dq, dr]) => a.q + dq === b.q && a.r + dr === b.r)
  )
    throw new Error(`Invalid map edge ${aId}/${bId}`)
  return { a, b, key: edgeKey(a.id, b.id) }
}
export function parseLayers(
  lines: string,
  sides: string,
  coverage: string,
  membership = HEX_BY_ID,
): Layers {
  const masks = new Map<Layer, Set<string>>()
  for (const row of rows(coverage)) {
    const layer = row.layer as Layer
    if (!COVERAGE_LAYERS.includes(layer))
      throw new Error(`Unknown coverage layer ${layer}`)
    const hex = membership.get(row.hex_id)
    if (!hex) throw new Error(`Unknown coverage hex ${row.hex_id}`)
    const key = row.neighbour_id
      ? resolveEdge(row.hex_id, row.neighbour_id, membership).key
      : hex.id
    if (layer.includes(':') !== Boolean(row.neighbour_id))
      throw new Error('Map coverage cell/edge mismatch')
    if (!masks.has(layer)) masks.set(layer, new Set())
    masks.get(layer)!.add(key)
  }
  const features: Feature[] = [
    ...rows(lines).map((r) => ({
      layer: `line:${r.kind}` as Layer,
      edge: resolveEdge(r.from_hex, r.to_hex, membership),
      src: r.src,
    })),
    ...rows(sides).map((r) => ({
      layer: `side:${r.feature}` as Layer,
      edge: resolveEdge(r.hex_id, r.neighbour_id, membership),
      highSide: membership.get(r.high_side)?.id,
      src: r.src,
    })),
  ]
  for (const f of features) {
    if (
      !FEATURE_LAYERS.includes(f.layer) ||
      !masks.get(f.layer)?.has(f.edge.key)
    )
      throw new Error(
        `Feature outside explicit coverage: ${f.layer}/${f.edge.key}`,
      )
    if (
      ['side:slope', 'side:escarpment'].includes(f.layer) &&
      f.highSide !== f.edge.a.id &&
      f.highSide !== f.edge.b.id
    )
      throw new Error('Directional feature has no published high side')
  }
  return { features, coverage: masks }
}
export function status(layers: Layers, layer: Layer, key: string) {
  return !layers.coverage.get(layer)?.has(key)
    ? 'unknown'
    : layers.features.some((f) => f.layer === layer && f.edge.key === key)
      ? 'present'
      : 'surveyed: none'
}
/** Shared hex boundary, derived from axial geometry; high-side symbols point downhill. */
export function boundary(edge: Edge) {
  const a = center(edge.a),
    b = center(edge.b),
    dx = b.x - a.x,
    dy = b.y - a.y,
    d = Math.hypot(dx, dy)
  const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 },
    tangent = { x: -dy / d, y: dx / d }
  return {
    mid,
    tangent,
    normal: { x: dx / d, y: dy / d },
    start: {
      x: mid.x - (tangent.x * HEX_SIZE) / 2,
      y: mid.y - (tangent.y * HEX_SIZE) / 2,
    },
    end: {
      x: mid.x + (tangent.x * HEX_SIZE) / 2,
      y: mid.y + (tangent.y * HEX_SIZE) / 2,
    },
  }
}
export const LAYER_FIXTURE =
  import.meta.env.DEV &&
  typeof location !== 'undefined' &&
  new URLSearchParams(location.search).get('layers') === 'fixture'
export function syntheticLayers(): Layers {
  const coverage = new Map<Layer, Set<string>>(),
    features: Feature[] = []
  for (const layer of FEATURE_LAYERS) coverage.set(layer, new Set())
  EDGES.forEach((edge, i) => {
    if (i % 2 === 0) coverage.get('line:road')!.add(edge.key)
    if (i % 10 !== 0) return
    const layer = FEATURE_LAYERS[(i / 10) % FEATURE_LAYERS.length]
    coverage.get(layer)!.add(edge.key)
    features.push({
      layer,
      edge,
      highSide:
        layer === 'side:slope' || layer === 'side:escarpment'
          ? edge.a.id
          : undefined,
      src: '',
    })
  })
  return { coverage, features }
}
export const MAP_LAYERS: Layers = LAYER_FIXTURE
  ? syntheticLayers()
  : SYNTHETIC
    ? { features: [], coverage: new Map() }
    : parseLayers(lineCsv, sideCsv, coverageCsv)
