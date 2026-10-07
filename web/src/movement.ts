import type { PendingDecision } from './generated/PendingDecision'
import type { JsonValue } from './generated/serde_json/JsonValue'
import type { ViewState } from './protocol'
import { segmentKey, type Frame } from './stream/model'
import { center, HEX_BY_ID } from './map/fixture'
export interface Point {
  x: number
  y: number
}
export interface Motion {
  key: string
  unitId?: string
  cpSpent?: number
  points: Point[]
}
function object(
  value: JsonValue | undefined,
): Record<string, JsonValue> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value
    : undefined
}
/** Read only the published movement list schema. Other decisions are not guessed to be movement. */
export function movementUnits(decision: PendingDecision): string[] | null {
  if (decision.kind !== 'cna.movement.orders') return null
  const schema = object(decision.space)
  const alternatives = Array.isArray(schema?.anyOf)
    ? schema.anyOf
    : [decision.space]
  for (const alternative of alternatives) {
    const list = object(alternative),
      item = object(list?.items),
      properties = object(item?.properties),
      unit = object(properties?.unit)
    if (
      list?.type === 'array' &&
      Array.isArray(unit?.enum) &&
      unit.enum.every((id) => typeof id === 'string')
    )
      return [...new Set(unit.enum as string[])]
  }
  return null
}
export function movedUnits(
  view: ViewState | undefined,
  frames: Frame[],
  seq: number | null,
): Set<string> {
  const moved = new Set<string>()
  if (!view) return moved
  const selected = frames.find((f) => f.seq === seq)
  if (selected?.moved)
    return new Set(selected.moved.filter((id) => Boolean(view.units[id])))
  const retained = frames.filter((f) => f.seq <= (seq ?? 0)),
    segment = segmentKey(view.clock)
  let start = retained.length - 1
  while (start > 0 && segmentKey(retained[start - 1].view.clock) === segment)
    start--
  const window = retained.slice(Math.max(0, start))
  // Seed from a snapshot or an evicted prefix, never from stale flags across a phase boundary.
  const seed = !window.length
    ? view
    : start === 0 || window[0].event === null
      ? window[0].view
      : undefined
  if (seed)
    for (const unit of Object.values(seed.units))
      if (unit.detail?.moved_this_segment === true) moved.add(unit.id)
  for (const frame of window) {
    if (segmentKey(frame.view.clock) !== segment) continue
    const event = frame.event
    if (event?.kind === 'unit_moved') moved.add(event.unit_id)
    else if (event?.kind === 'unit_updated') {
      if (event.unit.detail?.moved_this_segment === true)
        moved.add(event.unit.id)
      else if (event.unit.detail?.moved_this_segment === false)
        moved.delete(event.unit.id)
    } else if (event?.kind === 'unit_removed') moved.delete(event.unit_id)
  }
  for (const id of moved) if (!view.units[id]) moved.delete(id)
  return moved
}
/** Origin comes from the preceding projected frame; an absent unit never acquires an inferred route. */
export function motionEvents(
  frames: Frame[],
  previous: number | null,
  seq: number | null,
  allowBatch: boolean,
): Motion[] {
  if (
    previous === null ||
    seq === null ||
    seq <= previous ||
    (!allowBatch && seq !== previous + 1)
  )
    return []
  const index = frames.findIndex((f) => f.seq === previous)
  if (index < 0) return []
  const results = new Map<string, Motion>()
  for (let i = index + 1; i < frames.length && frames[i].seq <= seq; i++) {
    const frame = frames[i],
      event = frame.event
    if (event?.kind === 'unit_moved') {
      const unit = frames[i - 1].view.units[event.unit_id],
        origin = unit?.hex && HEX_BY_ID.get(unit.hex)
      if (!event.path.length || event.path.length > 4096) continue
      const route = event.path.map((id) => HEX_BY_ID.get(id))
      if (
        !origin ||
        !route.length ||
        route.length > 4096 ||
        route.some((hex) => !hex)
      )
        continue
      const key = `unit:${event.unit_id}`,
        points = [origin, ...route].map((h) => center(h!))
      results.set(key, {
        key,
        unitId: event.unit_id,
        points,
        ...(typeof event.cp_spent === 'number' &&
        Number.isFinite(event.cp_spent)
          ? { cpSpent: event.cp_spent }
          : {}),
      })
    } else if (event?.kind === 'unit_updated') {
      const previousUnit = frames[i - 1].view.units[event.unit.id]
      const location = previousUnit?.detail?.location
      const isAwaiting =
        location &&
        typeof location === 'object' &&
        !Array.isArray(location) &&
        location.at === 'awaiting_setup'
      const destination = event.unit.hex && HEX_BY_ID.get(event.unit.hex)
      if (isAwaiting && !previousUnit.hex && destination) {
        const key = `unit:${event.unit.id}`
        results.set(key, {
          key,
          unitId: event.unit.id,
          points: [center(destination)],
        })
      }
    } else if (event?.kind === 'marker_placed') {
      const destination = HEX_BY_ID.get(event.marker.hex)
      if (destination)
        results.set(`marker:${event.marker.id}`, {
          key: `marker:${event.marker.id}`,
          points: [center(destination)],
        })
    } else if (
      event?.kind === 'stack_updated' ||
      event?.kind === 'stack_removed'
    ) {
      const id = event.kind === 'stack_updated' ? event.stack.hex : event.hex,
        hex = HEX_BY_ID.get(id)
      if (!hex) continue
      const key = `stack:${id}:${event.kind === 'stack_updated' ? event.stack.side : event.side}`
      results.set(key, { key, points: [center(hex)] })
    }
  }
  return [...results.values()].slice(-128)
}
/** Constant speed over all disclosed path legs, rather than a straight line through intervening hexes. */
export function routeSampler(points: Point[]): (progress: number) => Point {
  const cumulative = [0]
  for (let i = 1; i < points.length; i++)
    cumulative.push(
      cumulative[i - 1] +
        Math.hypot(
          points[i].x - points[i - 1].x,
          points[i].y - points[i - 1].y,
        ),
    )
  const total = cumulative.at(-1)!
  return (progress) => {
    if (points.length === 1 || !total) return points[0]
    const distance = total * Math.max(0, Math.min(1, progress))
    let low = 1,
      high = cumulative.length - 1
    while (low < high) {
      const mid = (low + high) >>> 1
      if (cumulative[mid] < distance) low = mid + 1
      else high = mid
    }
    const length = cumulative[low] - cumulative[low - 1],
      t = length ? (distance - cumulative[low - 1]) / length : 0
    return {
      x: points[low - 1].x + (points[low].x - points[low - 1].x) * t,
      y: points[low - 1].y + (points[low].y - points[low - 1].y) * t,
    }
  }
}
