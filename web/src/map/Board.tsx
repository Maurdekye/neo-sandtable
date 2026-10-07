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
import { createMotions } from './motion'
import { createPlacements } from './placements'
import { motionEvents } from '../movement'
import type { Frame } from '../stream/model'
import { counterSvg } from './counters'
import { createOverlays } from './overlay'
import { label, MAP_LAYERS, LAYER_FIXTURE, type LayerOptions } from './layers'
interface Props {
  view: ViewState | undefined
  selected: string | null
  focus: { hex: string; nonce: number; bounds?: string[] } | null
  onSelect: (hex: string) => void
  layers: LayerOptions
  frames: Frame[]
  seq: number | null
  scope: string
  moving: boolean
  allowBatch: boolean
  moved: Set<string>
  placement: { label: string; hexes: string[] } | null
}
interface Scene {
  app: Application
  world: Container
  counters: Container
  selection: Graphics
  textures: Map<string, Texture>
  updateVisibility: () => void
  overlays: ReturnType<typeof createOverlays>
  placements: ReturnType<typeof createPlacements>
  motions: ReturnType<typeof createMotions>
  motionCursor: { scope: string; seq: number | null }
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
export function Board({
  view,
  selected,
  focus,
  onSelect,
  layers,
  frames: history,
  seq,
  scope,
  moving,
  allowBatch,
  moved,
  placement,
}: Props) {
  const host = useRef<HTMLDivElement>(null),
    scene = useRef<Scene | null>(null),
    select = useRef(onSelect)
  const [ready, setReady] = useState(false),
    [fps, setFps] = useState(0),
    [motionCount, setMotionCount] = useState(0),
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
        overlay = new Container(),
        placementLayer = new Container(),
        counters = new Container(),
        motion = new Container(),
        selection = new Graphics()
      world.addChild(
        terrain,
        overlay,
        placementLayer,
        counters,
        motion,
        selection,
      )
      const motions = createMotions(motion)
      const placements = createPlacements(placementLayer)
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
        chunk.minX = Math.min(chunk.minX, p.x - 52)
        chunk.maxX = Math.max(chunk.maxX, p.x + 52)
        chunk.minY = Math.min(chunk.minY, p.y - 52)
        chunk.maxY = Math.max(chunk.maxY, p.y + 52)
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
      const overlays = createOverlays(terrain, chunks)
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
        placements.visibility(
          scale,
          world.x,
          world.y,
          element.clientWidth,
          element.clientHeight,
        )
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
        overlays,
        placements,
        motions,
        motionCursor: { scope: '', seq: null },
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
          overlays.resolution(band)
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
        motions.tick(now)
        const count = motions.count()
        setMotionCount((old) => (old === count ? old : count))
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
        placements.clear()
        motions.clear()
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
    if (!s || !ready) return
    s.placements.build(placement?.hexes ?? [])
    s.updateVisibility()
  }, [placement, ready])
  useEffect(() => {
    const s = scene.current
    if (!s || !ready) return
    s.overlays.build(layers)
    s.updateVisibility()
  }, [layers, ready])
  useEffect(() => {
    const s = scene.current
    if (!s || !ready) return
    if (!view) {
      s.motions.clear()
      s.motions.sprites(new Map())
      s.counters.removeChildren().forEach((c) => c.destroy())
      return
    }
    let canceled = false
    async function draw() {
      const units = Object.values(view!.units)
      await Promise.all(
        [...new Set(units.map(counterSvg))].map(async (key) => {
          if (!s!.textures.has(key)) {
            const texture = await rasterize(key)
            if (canceled) texture.destroy(true)
            else s!.textures.set(key, texture)
          }
        }),
      )
      if (canceled) return
      s!.motions.sprites(new Map())
      s!.counters.removeChildren().forEach((c) => c.destroy())
      const active = new Set(units.map(counterSvg))
      // Bound retired texture variants; textures still displayed remain alive.
      for (const [key, texture] of s!.textures) {
        if (s!.textures.size <= 256) break
        if (!active.has(key) && !s!.motions.textures().has(texture)) {
          s!.textures.delete(key)
          texture.destroy(true)
        }
      }
      const unitSprites = new Map<string, Sprite>()
      view!.markers.forEach((marker) => {
        const hex = HEX_BY_ID.get(marker.hex)
        if (!hex) return
        const p = center(hex),
          color =
            marker.side === 'axis'
              ? 0xdec08c
              : marker.side === 'commonwealth'
                ? 0x8ebbbb
                : 0xf5dfa1
        const glyph = new Graphics()
          .poly([
            p.x,
            p.y + 17,
            p.x + 8,
            p.y + 25,
            p.x,
            p.y + 33,
            p.x - 8,
            p.y + 25,
          ])
          .fill({ color: 0x172d35, alpha: 0.9 })
          .stroke({ color, width: 2 })
        s!.counters.addChild(glyph)
        const label = new Text({
          text: marker.label ?? marker.kind,
          style: {
            fontFamily: 'sans-serif',
            fontSize: 9,
            fill: color,
            stroke: { color: 0x172d35, width: 2 },
          },
        })
        label.position.set(p.x + 11, p.y + 22)
        s!.counters.addChild(label)
      })
      view!.stacks.forEach((stack) => {
        const h = HEX_BY_ID.get(stack.hex)
        if (!h) return
        const p = center(h),
          expanded = Boolean(selected && HEX_BY_ID.get(selected)?.id === h.id)
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
        stack.unit_ids.slice(0, expanded ? 20 : 3).forEach((id, i) => {
          const u = view!.units[id]
          if (!u) return
          const texture = s!.textures.get(counterSvg(u))
          if (!texture) return
          const sprite = new Sprite(texture)
          sprite.width = 38
          sprite.height = 31
          sprite.position.set(
            p.x - 19 + (expanded ? (i % 4) * 42 : i * 3),
            p.y - 16 + (expanded ? Math.floor(i / 4) * 35 : -i * 3),
          )
          s!.counters.addChild(sprite)
          unitSprites.set(id, sprite)
          if (moved.has(id)) {
            const flag = new Graphics().circle(0, 0, 3).fill(0x8be4c0)
            flag.position.set(sprite.x + 3, sprite.y + 3)
            s!.counters.addChild(flag)
          }
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
      s!.motions.sprites(unitSprites)
    }
    void draw().catch((e) => setError(String(e)))
    return () => {
      canceled = true
    }
  }, [view, selected, ready, moved])
  useEffect(() => {
    const s = scene.current
    if (!s || !ready) return
    if (
      s.motionCursor.scope !== scope ||
      !view ||
      !moving ||
      (seq ?? 0) < (s.motionCursor.seq ?? 0)
    )
      s.motions.clear()
    else if (seq !== s.motionCursor.seq) {
      const events = motionEvents(history, s.motionCursor.seq, seq, allowBatch)
      if (
        !events.length &&
        ((!allowBatch && seq! > (s.motionCursor.seq ?? 0) + 1) ||
          !history.some((f) => f.seq === s.motionCursor.seq))
      )
        s.motions.clear()
      else
        s.motions.start(
          events.filter(
            (event) => !event.unitId || Boolean(view.units[event.unitId]),
          ),
          (id) => {
            const unit = view.units[id]
            return unit ? s.textures.get(counterSvg(unit)) : undefined
          },
        )
    }
    s.motionCursor = { scope, seq }
  }, [history, seq, scope, moving, allowBatch, view, ready])
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
    let p = center(h)
    const points =
      focus?.bounds?.flatMap((id) => {
        const hex = HEX_BY_ID.get(id)
        return hex ? [center(hex)] : []
      }) ?? []
    if (points.length) {
      const xs = points.map((p) => p.x),
        ys = points.map((p) => p.y),
        minX = Math.min(...xs),
        maxX = Math.max(...xs),
        minY = Math.min(...ys),
        maxY = Math.max(...ys)
      p = { x: (minX + maxX) / 2, y: (minY + maxY) / 2 }
      s.world.scale.set(
        Math.max(
          0.35,
          Math.min(
            1.25,
            (host.current!.clientWidth - 160) / (maxX - minX + 100),
            (host.current!.clientHeight - 180) / (maxY - minY + 100),
          ),
        ),
      )
    }
    if (!points.length && s.world.scale.x < 0.85) s.world.scale.set(0.85)
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
      aria-label="Hex map; drag to pan, wheel to zoom"
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
      {placement && (
        <output className="placement-caption" data-testid="placement-highlight">
          {placement.hexes.length} legal set-up hexes - {placement.label}
        </output>
      )}
      <output className="motion-caption" data-testid="motion-count">
        {motionCount} active animations - green dot: moved this segment
      </output>
      {error && <div className="map-error">Renderer unavailable: {error}</div>}
      <div className="layer-caption" data-testid="layer-caption">
        {LAYER_FIXTURE ? 'SYNTHETIC LAYER FIXTURE - ' : ''}
        {MAP_LAYERS.features.length} {LAYER_FIXTURE ? 'generated' : 'surveyed'}{' '}
        features -{' '}
        {layers.coverage
          ? `Hatch: unknown ${label(layers.coverage)}`
          : 'Coverage hatch off'}
      </div>
      <div className="legend">
        {Object.entries(TERRAIN)
          .filter(([key]) => HEXES.some((h) => h.terrain === key))
          .map(([key, t]) => (
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
