import { useEffect, useRef, useState } from 'react'
import {
  Application,
  Container,
  Graphics,
  Sprite,
  Text,
  Texture,
} from 'pixi.js'
import type { ViewState } from '../protocol'
import {
  center,
  HEXES,
  MAP_LABEL,
  INITIAL_HEX,
  HEX_BY_ID,
  hexAt,
  TERRAIN,
  vertices,
} from './fixture'
import { counterSvg } from './counters'
interface Props {
  view: ViewState | undefined
  selected: string | null
  focus: { hex: string; nonce: number } | null
  onSelect: (hex: string) => void
}
interface Scene {
  app: Application
  world: Container
  counters: Container
  selection: Graphics
  textures: Map<string, Texture>
  updateVisibility: () => void
}
async function rasterize(svg: string): Promise<Texture> {
  const image = new Image()
  image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
  await image.decode()
  const canvas = document.createElement('canvas')
  canvas.width = 120
  canvas.height = 96
  canvas.getContext('2d')!.drawImage(image, 0, 0, 120, 96)
  return Texture.from(canvas)
}
export function Board({ view, selected, focus, onSelect }: Props) {
  const host = useRef<HTMLDivElement>(null),
    scene = useRef<Scene | null>(null),
    select = useRef(onSelect)
  const [ready, setReady] = useState(false),
    [fps, setFps] = useState(0),
    [error, setError] = useState('')
  select.current = onSelect
  useEffect(() => {
    const element = host.current!
    const app = new Application()
    let disposed = false,
      initialized = false,
      cleanup = () => {}
    async function init() {
      await app.init({
        resizeTo: element,
        background: 0x10252d,
        antialias: true,
        resolution: Math.min(devicePixelRatio, 2),
        autoDensity: true,
        preference: 'webgl',
      })
      initialized = true
      if (disposed) {
        app.destroy(true, { children: true })
        return
      }
      element.appendChild(app.canvas)
      const world = new Container(),
        terrain = new Container(),
        counters = new Container(),
        selection = new Graphics()
      world.addChild(terrain, counters, selection)
      app.stage.addChild(world)
      const chunks = new Map<
        string,
        {
          container: Container
          graphics: Graphics
          minX: number
          minY: number
          maxX: number
          maxY: number
        }
      >()
      HEXES.forEach((h) => {
        const key = `${Math.floor(h.q / 10)}-${Math.floor(h.r / 10)}`
        let chunk = chunks.get(key)
        if (!chunk) {
          const container = new Container(),
            graphics = new Graphics()
          container.addChild(graphics)
          terrain.addChild(container)
          chunk = {
            container,
            graphics,
            minX: Infinity,
            minY: Infinity,
            maxX: -Infinity,
            maxY: -Infinity,
          }
          chunks.set(key, chunk)
        }
        const p = center(h)
        chunk.minX = Math.min(chunk.minX, p.x - 27)
        chunk.maxX = Math.max(chunk.maxX, p.x + 27)
        chunk.minY = Math.min(chunk.minY, p.y - 27)
        chunk.maxY = Math.max(chunk.maxY, p.y + 27)
        chunk.graphics
          .poly(vertices(h))
          .fill(TERRAIN[h.terrain].color)
          .stroke({ color: 0x35413a, alpha: 0.24, width: 0.7 })
        if (h.terrain === 'mountain')
          chunk.graphics
            .poly([p.x - 8, p.y + 5, p.x, p.y - 8, p.x + 8, p.y + 5])
            .fill({ color: 0xddd2af, alpha: 0.4 })
      })
      chunks.forEach((c) => c.container.cacheAsTexture({ resolution: 1 }))
      HEXES.filter((h) => h.label).forEach((h) => {
        const p = center(h)
        const text = new Text({
          text: h.label,
          style: { fontFamily: 'sans-serif', fontSize: 9, fill: 0xeae5c7 },
        })
        text.position.set(p.x - 22, p.y + 17)
        world.addChild(text)
      })
      const start = center(HEX_BY_ID.get(INITIAL_HEX)!)
      world.scale.set(1.25)
      world.position.set(
        element.clientWidth / 2 - start.x * 1.25,
        element.clientHeight / 2 - start.y * 1.25,
      )
      const updateVisibility = () => {
        const scale = world.scale.x
        chunks.forEach((c) => {
          c.container.renderable =
            c.maxX * scale + world.x > 0 &&
            c.minX * scale + world.x < element.clientWidth &&
            c.maxY * scale + world.y > 0 &&
            c.minY * scale + world.y < element.clientHeight
        })
      }
      scene.current = {
        app,
        world,
        counters,
        selection,
        textures: new Map(),
        updateVisibility,
      }
      updateVisibility()
      let dragging = false,
        moved = false,
        lastX = 0,
        lastY = 0,
        downX = 0,
        downY = 0,
        band = 1
      const down = (e: PointerEvent) => {
        dragging = true
        moved = false
        lastX = downX = e.clientX
        lastY = downY = e.clientY
        app.canvas.setPointerCapture(e.pointerId)
      }
      const move = (e: PointerEvent) => {
        if (!dragging) return
        moved ||= Math.hypot(e.clientX - downX, e.clientY - downY) > 5
        world.x += e.clientX - lastX
        world.y += e.clientY - lastY
        lastX = e.clientX
        lastY = e.clientY
        updateVisibility()
      }
      const up = (e: PointerEvent) => {
        if (!dragging) return
        dragging = false
        if (!moved) {
          const rect = app.canvas.getBoundingClientRect()
          const h = hexAt(
            (e.clientX - rect.left - world.x) / world.scale.x,
            (e.clientY - rect.top - world.y) / world.scale.y,
          )
          if (h) select.current(h.id)
        }
      }
      const wheel = (e: WheelEvent) => {
        e.preventDefault()
        const rect = app.canvas.getBoundingClientRect(),
          x = e.clientX - rect.left,
          y = e.clientY - rect.top
        const old = world.scale.x,
          next = Math.max(0.25, Math.min(3, old * Math.exp(-e.deltaY * 0.0015)))
        world.position.set(
          x - ((x - world.x) * next) / old,
          y - ((y - world.y) * next) / old,
        )
        world.scale.set(next)
        const nextBand = next < 0.65 ? 0.5 : next > 1.8 ? 2 : 1
        if (nextBand !== band) {
          band = nextBand
          chunks.forEach((c) => {
            c.container.cacheAsTexture(false)
            c.container.cacheAsTexture({ resolution: band })
          })
        }
        updateVisibility()
      }
      app.canvas.addEventListener('pointerdown', down)
      app.canvas.addEventListener('pointermove', move)
      app.canvas.addEventListener('pointerup', up)
      app.canvas.addEventListener('pointercancel', up)
      app.canvas.addEventListener('wheel', wheel, { passive: false })
      const observer = new ResizeObserver(updateVisibility)
      observer.observe(element)
      let frames = 0,
        last = performance.now()
      app.ticker.add(() => {
        frames++
        const now = performance.now()
        if (now - last >= 1000) {
          setFps(Math.round((frames * 1000) / (now - last)))
          last = now
          frames = 0
        }
      })
      cleanup = () => {
        observer.disconnect()
        app.canvas.removeEventListener('pointerdown', down)
        app.canvas.removeEventListener('pointermove', move)
        app.canvas.removeEventListener('pointerup', up)
        app.canvas.removeEventListener('pointercancel', up)
        app.canvas.removeEventListener('wheel', wheel)
        scene.current?.textures.forEach((t) => t.destroy(true))
        scene.current = null
        app.destroy(true, { children: true })
      }
      setReady(true)
    }
    void init().catch((e) => {
      if (!disposed) setError(String(e))
    })
    return () => {
      disposed = true
      if (initialized) cleanup()
    }
  }, [])
  useEffect(() => {
    const s = scene.current
    if (!s || !view || !ready) return
    let canceled = false
    async function draw() {
      const units = Object.values(view!.units)
      await Promise.all(
        units.map(async (u) => {
          const key = counterSvg(u)
          if (!s!.textures.has(key)) {
            const texture = await rasterize(key)
            if (canceled) texture.destroy(true)
            else s!.textures.set(key, texture)
          }
        }),
      )
      if (canceled) return
      s!.counters.removeChildren().forEach((c) => c.destroy())
      const active = new Set(units.map(counterSvg))
      // Bound retired texture variants; textures still displayed remain alive.
      for (const [key, texture] of s!.textures) {
        if (s!.textures.size <= 256) break
        if (!active.has(key)) {
          s!.textures.delete(key)
          texture.destroy(true)
        }
      }
      view!.stacks.forEach((stack) => {
        const h = HEX_BY_ID.get(stack.hex)
        if (!h) return
        const p = center(h),
          expanded = selected === stack.hex
        if (!stack.unit_ids.length) {
          const marker = new Graphics()
            .roundRect(-17, -13, 34, 26, 3)
            .fill(stack.side === 'axis' ? 0xdbc8a0 : 0x8ebbbb)
            .stroke({ color: 0x172d35, width: 2 })
          marker.position.set(p.x, p.y)
          s!.counters.addChild(marker)
          const text = new Text({
            text: '?',
            style: { fontSize: 18, fill: 0x172d35 },
          })
          text.position.set(p.x - 5, p.y - 10)
          s!.counters.addChild(text)
          return
        }
        stack.unit_ids.forEach((id, i) => {
          const u = view!.units[id]
          if (!u) return
          const texture = s!.textures.get(counterSvg(u))
          if (!texture) return
          const sprite = new Sprite(texture)
          sprite.width = 38
          sprite.height = 31
          sprite.position.set(
            p.x - 19 + (expanded ? i * 24 : i * 3),
            p.y - 16 - (expanded ? i * 7 : i * 3),
          )
          s!.counters.addChild(sprite)
        })
        if (stack.unit_ids.length > 1) {
          const badge = new Text({
            text: String(stack.visible_count ?? '?'),
            style: {
              fontFamily: 'sans-serif',
              fontSize: 11,
              fontWeight: 'bold',
              fill: 0xffffff,
              stroke: { color: 0x172d35, width: 3 },
            },
          })
          badge.position.set(p.x + 18, p.y + 12)
          s!.counters.addChild(badge)
        }
      })
    }
    void draw().catch((e) => setError(String(e)))
    return () => {
      canceled = true
    }
  }, [view, selected, ready])
  useEffect(() => {
    const s = scene.current
    if (!s || !ready) return
    s.selection.clear()
    const h = selected ? HEX_BY_ID.get(selected) : undefined
    if (h) s.selection.poly(vertices(h)).stroke({ color: 0xffefb5, width: 3 })
  }, [selected, ready])
  useEffect(() => {
    const s = scene.current,
      h = focus ? HEX_BY_ID.get(focus.hex) : undefined
    if (!s || !h || !ready) return
    const p = center(h)
    s.world.position.set(
      host.current!.clientWidth / 2 - p.x * s.world.scale.x,
      host.current!.clientHeight / 2 - p.y * s.world.scale.y,
    )
    s.updateVisibility()
  }, [focus, ready])
  return (
    <div
      className="board"
      ref={host}
      role="img"
      aria-label="Synthetic hex map; drag to pan, wheel to zoom"
    >
      <div className="map-caption">
        {MAP_LABEL}{' '}
        <span>
          {HEXES.length.toLocaleString()} hexes · drag to pan · scroll to zoom
        </span>
      </div>
      <output className="fps" data-testid="fps">
        {fps} FPS · WebGL
      </output>
      {error && <div className="map-error">Renderer unavailable: {error}</div>}
      <div className="legend">
        {Object.entries(TERRAIN).map(([key, t]) => (
          <span key={key}>
            <i
              style={{
                background: `#${t.color.toString(16).padStart(6, '0')}`,
              }}
            />
            {t.label}
          </span>
        ))}
      </div>
    </div>
  )
}
