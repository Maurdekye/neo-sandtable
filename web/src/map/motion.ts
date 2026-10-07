import { Container, Graphics, Sprite, Texture, Text } from 'pixi.js'
import { routeSampler, type Motion } from '../movement'
/** Presentation only: the stream reducer applies the adjudicated positions immediately. */
export function createMotions(parent: Container) {
  const active = new Map<
    string,
    {
      motion: Motion
      root: Container
      glyph: Container
      start: number
      duration: number
      sample: ReturnType<typeof routeSampler>
    }
  >()
  let sprites = new Map<string, Sprite>()
  const finish = (key: string) => {
    const item = active.get(key)
    if (!item) return
    item.root.destroy({ children: true })
    if (item.motion.unitId) {
      const sprite = sprites.get(item.motion.unitId)
      if (sprite) sprite.visible = true
    }
    active.delete(key)
  }
  return {
    count: () => active.size,
    textures: () =>
      new Set(
        [...active.values()].flatMap((item) =>
          item.glyph.children.flatMap((child) =>
            child instanceof Sprite ? [child.texture] : [],
          ),
        ),
      ),
    clear: () => {
      ;[...active.keys()].forEach(finish)
    },
    sprites: (next: Map<string, Sprite>) => {
      sprites = next
      for (const item of active.values())
        if (item.motion.unitId) {
          const sprite = sprites.get(item.motion.unitId)
          if (sprite) sprite.visible = false
        }
    },
    start: (
      motions: Motion[],
      texture: (id: string) => Texture | undefined,
    ) => {
      for (const motion of motions) {
        finish(motion.key)
        if (active.size >= 128) finish(active.keys().next().value!)
        const root = new Container(),
          glyph = new Container(),
          route = new Graphics()
        parent.addChild(root)
        root.addChild(route, glyph)
        const points = motion.points
        if (points.length > 1) {
          route.moveTo(points[0].x, points[0].y)
          points.slice(1).forEach((p) => route.lineTo(p.x, p.y))
          route.stroke({ color: 0x8be4c0, width: 2, alpha: 0.65 })
          points.forEach((p) => route.circle(p.x, p.y, 2.5).fill(0x8be4c0))
        }
        const tex = motion.unitId ? texture(motion.unitId) : undefined
        if (tex) {
          const sprite = new Sprite(tex)
          sprite.width = 38
          sprite.height = 31
          sprite.position.set(-19, -16)
          glyph.addChild(sprite)
        } else
          glyph.addChild(
            new Graphics()
              .circle(0, 0, 21)
              .stroke({ color: 0x8be4c0, width: 2 }),
          )
        if (motion.cpSpent !== undefined) {
          const label = new Text({
            text: `${motion.cpSpent} CP`,
            style: {
              fontFamily: 'sans-serif',
              fontSize: 11,
              fill: 0xffffff,
              stroke: { color: 0x172d35, width: 3 },
            },
          })
          label.position.set(-18, -32)
          glyph.addChild(label)
        }
        if (motion.unitId) {
          const sprite = sprites.get(motion.unitId)
          if (sprite) sprite.visible = false
        }
        const duration =
          points.length === 1
            ? 700
            : Math.min(3600, 700 + (points.length - 2) * 240)
        glyph.position.copyFrom(points[0])
        active.set(motion.key, {
          motion,
          root,
          glyph,
          start: performance.now(),
          duration,
          sample: routeSampler(points),
        })
      }
    },
    tick: (now: number) => {
      for (const [key, item] of active) {
        const t = (now - item.start) / item.duration
        if (t >= 1) {
          finish(key)
          continue
        }
        const p = item.sample(t)
        item.glyph.position.set(p.x, p.y)
        if (item.motion.points.length === 1) {
          item.glyph.scale.set(0.7 + t * 0.7)
          item.root.alpha = 1 - t
        }
      }
    },
  }
}
