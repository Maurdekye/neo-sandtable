import { Container, Graphics } from 'pixi.js'
import { HEXES, center, vertices, HEX_SIZE } from './fixture'
import { terrainCoverage } from './terrainCoverage'
/** Classification and corridor are independent dimensions. Static chunks cull with the map. */
export function createTerrainOverlay(parent: Container) {
  const chunks = new Map<
    string,
    { g: Graphics; minX: number; maxX: number; minY: number; maxY: number }
  >()
  for (const h of HEXES) {
    const key = `${Math.floor(h.q / 10)},${Math.floor(h.r / 10)}`,
      p = center(h),
      c = terrainCoverage(h)
    let chunk = chunks.get(key)
    if (!chunk) {
      const g = new Graphics()
      parent.addChild(g)
      chunk = {
        g,
        minX: Infinity,
        maxX: -Infinity,
        minY: Infinity,
        maxY: -Infinity,
      }
      chunks.set(key, chunk)
    }
    chunk.minX = Math.min(chunk.minX, p.x - HEX_SIZE)
    chunk.maxX = Math.max(chunk.maxX, p.x + HEX_SIZE)
    chunk.minY = Math.min(chunk.minY, p.y - HEX_SIZE)
    chunk.maxY = Math.max(chunk.maxY, p.y + HEX_SIZE)
    if (c.classified)
      chunk.g.poly(vertices(h)).fill({ color: 0x8be4c0, alpha: 0.1 })
    else
      for (const offset of [-9, 0, 9])
        chunk.g
          .moveTo(p.x - 10, p.y + offset + 5)
          .lineTo(p.x + 10, p.y + offset - 5)
          .stroke({ color: 0xd3dcdf, alpha: 0.33, width: 0.8 })
    if (c.corridor)
      chunk.g
        .poly(vertices(h))
        .stroke({ color: 0x6dbce8, alpha: 0.85, width: 1.5 })
  }
  parent.visible = false
  return {
    toggle: (enabled: boolean) => {
      parent.visible = enabled
    },
    visibility: (scale: number, x: number, y: number, w: number, h: number) => {
      chunks.forEach((c) => {
        c.g.renderable =
          c.maxX * scale + x > 0 &&
          c.minX * scale + x < w &&
          c.maxY * scale + y > 0 &&
          c.minY * scale + y < h
      })
    },
    clear: () => {
      chunks.forEach((c) => c.g.destroy())
      chunks.clear()
    },
  }
}
