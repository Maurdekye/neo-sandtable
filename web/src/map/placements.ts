import { Container, Graphics } from 'pixi.js'
import { center, HEX_BY_ID, HEX_SIZE, vertices } from './fixture'
/** Chunked highlight over only the authorized legal destinations. No hidden chosen position is drawn. */
export function createPlacements(parent: Container) {
  const chunks = new Map<
    string,
    {
      graphic: Graphics
      minX: number
      maxX: number
      minY: number
      maxY: number
    }
  >()
  function clear() {
    chunks.forEach((chunk) => chunk.graphic.destroy())
    chunks.clear()
  }
  return {
    clear,
    build(ids: string[]) {
      clear()
      for (const id of ids) {
        const hex = HEX_BY_ID.get(id)
        if (!hex) continue
        const key = `${Math.floor(hex.q / 10)},${Math.floor(hex.r / 10)}`,
          p = center(hex)
        let chunk = chunks.get(key)
        if (!chunk) {
          const graphic = new Graphics()
          parent.addChild(graphic)
          chunk = {
            graphic,
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
        chunk.graphic
          .poly(vertices(hex))
          .fill({ color: 0x7ac9ee, alpha: 0.16 })
          .stroke({ color: 0x7ac9ee, width: 1.4, alpha: 0.8 })
      }
    },
    visibility(
      scale: number,
      x: number,
      y: number,
      width: number,
      height: number,
    ) {
      chunks.forEach((c) => {
        c.graphic.renderable =
          c.maxX * scale + x > 0 &&
          c.minX * scale + x < width &&
          c.maxY * scale + y > 0 &&
          c.minY * scale + y < height
      })
    },
  }
}
