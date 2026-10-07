import { Container, Graphics } from 'pixi.js'
import { center, HEXES, HEX_SIZE } from './fixture'
import {
  boundary,
  EDGES,
  MAP_LAYERS,
  type Feature,
  type LayerOptions,
} from './layers'
interface Chunk {
  container: Container
  graphics: Graphics
  minX: number
  maxX: number
  minY: number
  maxY: number
}
const styles: Record<string, { color: number; width: number }> = {
  road: { color: 0xecd9b0, width: 3 },
  track: { color: 0x86694f, width: 1.7 },
  railroad: { color: 0x17262b, width: 2 },
  pipeline: { color: 0x58b6b8, width: 2 },
  unfinished_road: { color: 0xecd9b0, width: 2 },
  unfinished_railroad: { color: 0x17262b, width: 2 },
  escarpment: { color: 0x483525, width: 2.2 },
  slope: { color: 0x756246, width: 1.8 },
  ridge: { color: 0x554433, width: 2 },
  wadi: { color: 0x557f83, width: 1.8 },
  minor_river: { color: 0x479eb2, width: 2 },
  major_river: { color: 0x368ca6, width: 3 },
  all_sea: { color: 0x418294, width: 2 },
  border: { color: 0xb9957b, width: 1.5 },
}
function drawFeature(g: Graphics, f: Feature) {
  const kind = f.layer.split(':')[1],
    style = styles[kind] ?? { color: 0xffffff, width: 1 }
  const b = boundary(f.edge)
  const start = f.layer.startsWith('line:') ? center(f.edge.a) : b.start,
    end = f.layer.startsWith('line:') ? center(f.edge.b) : b.end
  const dx = end.x - start.x,
    dy = end.y - start.y,
    length = Math.hypot(dx, dy),
    tx = dx / length,
    ty = dy / length,
    nx = -ty,
    ny = tx
  const dash =
    kind === 'track' || kind.startsWith('unfinished') || kind === 'border'
  if (dash)
    for (let t = 0; t < length; t += 7) {
      g.moveTo(start.x + tx * t, start.y + ty * t)
        .lineTo(
          start.x + tx * Math.min(t + 4, length),
          start.y + ty * Math.min(t + 4, length),
        )
        .stroke(style)
    }
  else g.moveTo(start.x, start.y).lineTo(end.x, end.y).stroke(style)
  if (kind.includes('railroad'))
    for (let t = 3; t < length; t += 6)
      g.moveTo(start.x + tx * t - nx * 3, start.y + ty * t - ny * 3)
        .lineTo(start.x + tx * t + nx * 3, start.y + ty * t + ny * 3)
        .stroke({ color: style.color, width: 1 })
  if (kind === 'pipeline')
    g.circle((start.x + end.x) / 2, (start.y + end.y) / 2, 2.5).stroke(style)
  if (kind === 'escarpment' || kind === 'slope') {
    const sign = f.highSide === f.edge.a.id ? 1 : -1
    for (let t = 3; t < length; t += 7) {
      const x = start.x + tx * t,
        y = start.y + ty * t
      g.moveTo(x, y)
        .lineTo(x + b.normal.x * sign * 5, y + b.normal.y * sign * 5)
        .stroke({ color: style.color, width: 1.5 })
    }
  }
  if (kind === 'ridge')
    for (let t = 3; t < length - 3; t += 8)
      g.poly([
        start.x + tx * (t - 2),
        start.y + ty * (t - 2),
        start.x + tx * t + nx * 3,
        start.y + ty * t + ny * 3,
        start.x + tx * (t + 2),
        start.y + ty * (t + 2),
      ]).stroke(style)
}
/** Cached, culled chunks. Coverage is one explicit per-kind lens, never inferred from other data. */
export function createOverlays(
  parent: Container,
  base: Map<string, { container: Container }>,
) {
  let chunks: Chunk[] = [],
    resolution = 1
  const build = (options: LayerOptions) => {
    chunks.forEach((c) => c.graphics.destroy())
    chunks = []
    const buckets = new Map<string, Chunk>()
    const obtain = (q: number, r: number, x: number, y: number) => {
      const key = `${Math.floor(q / 10)}-${Math.floor(r / 10)}`
      let c = buckets.get(key)
      if (!c) {
        const container = base.get(key)?.container ?? new Container(),
          graphics = new Graphics()
        container.addChild(graphics)
        if (!container.parent) parent.addChild(container)
        c = {
          container,
          graphics,
          minX: Infinity,
          maxX: -Infinity,
          minY: Infinity,
          maxY: -Infinity,
        }
        buckets.set(key, c)
      }
      c.minX = Math.min(c.minX, x - HEX_SIZE * 2)
      c.maxX = Math.max(c.maxX, x + HEX_SIZE * 2)
      c.minY = Math.min(c.minY, y - HEX_SIZE * 2)
      c.maxY = Math.max(c.maxY, y + HEX_SIZE * 2)
      return c.graphics
    }
    const layer = options.coverage,
      mask = layer ? MAP_LAYERS.coverage.get(layer) : undefined
    if (layer?.includes(':'))
      for (const edge of EDGES) {
        if (mask?.has(edge.key)) continue
        const b = boundary(edge),
          g = obtain(edge.a.q, edge.a.r, b.mid.x, b.mid.y)
        for (const t of [-7, 0, 7]) {
          const x = b.mid.x + b.tangent.x * t,
            y = b.mid.y + b.tangent.y * t
          g.moveTo(
            x - b.tangent.x * 2 - b.normal.x * 2,
            y - b.tangent.y * 2 - b.normal.y * 2,
          )
            .lineTo(
              x + b.tangent.x * 2 + b.normal.x * 2,
              y + b.tangent.y * 2 + b.normal.y * 2,
            )
            .stroke({ color: 0xe3d6b7, alpha: 0.28, width: 1 })
        }
      }
    else if (layer)
      for (const h of HEXES) {
        if (mask?.has(h.id)) continue
        const p = center(h),
          g = obtain(h.q, h.r, p.x, p.y)
        for (const t of [-9, 0, 9])
          g.moveTo(p.x + t - 4, p.y - 6)
            .lineTo(p.x + t + 4, p.y + 6)
            .stroke({ color: 0xe3d6b7, alpha: 0.23, width: 1 })
      }
    for (const f of MAP_LAYERS.features) {
      if (!options.visible.includes(f.layer)) continue
      const p = boundary(f.edge).mid
      drawFeature(obtain(f.edge.a.q, f.edge.a.r, p.x, p.y), f)
    }
    chunks = [...buckets.values()]
    base.forEach((c) => {
      c.container.cacheAsTexture(false)
      c.container.cacheAsTexture({ resolution })
    })
  }
  return {
    build,
    resolution: (next: number) => {
      resolution = next
    },
  }
}
