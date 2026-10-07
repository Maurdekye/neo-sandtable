import type { GameEvent } from './protocol'
import type { Frame } from './stream/model'
/** Only server-projected locators and the event's intrinsic location; never infer from history. */
export function eventHex(frame: Frame): string | null {
  if (frame.hex) return frame.hex
  const e = frame.event
  if (!e) return null
  switch (e.kind) {
    case 'unit_moved':
      return e.path.at(-1) ?? null
    case 'combat_resolved':
      return e.hex
    case 'stack_updated':
      return e.stack.hex
    case 'stack_removed':
      return e.hex
    case 'marker_placed':
      return e.marker.hex
    case 'unit_updated':
      return e.unit.hex
    default:
      return null
  }
}
export function eventText(e: GameEvent): string {
  switch (e.kind) {
    case 'unit_moved':
      return `${e.unit_id} > ${e.path.join(' > ')}${typeof e.cp_spent === 'number' ? ` · ${e.cp_spent} CP spent` : ''}`
    case 'combat_resolved':
    case 'decision_resolved':
      return e.summary
    case 'decision_opened':
      return e.decision.summary
    case 'note':
      return e.text
    case 'unit_removed':
      return e.reason
        ? `${e.unit_id} removed · ${e.reason}`
        : `${e.unit_id} counter no longer visible`
    case 'dice_rolled':
      return `${e.purpose} · dice ${e.dice.join(', ')}${e.reading !== null ? ` · reading ${e.reading}` : ''}`
    case 'unit_updated':
      return `${e.unit.name} updated`
    case 'stack_updated':
      return `${e.stack.side} stack updated at ${e.stack.hex}`
    case 'stack_removed':
      return `${e.side} stack removed at ${e.hex}`
    case 'marker_placed':
      return `${e.marker.kind} placed${e.marker.label ? ` · ${e.marker.label}` : ''}`
    case 'marker_removed':
      return `${e.marker_id} removed`
    case 'phase_changed':
      return `Phase · ${e.clock.segment ?? e.clock.phase}`
    default:
      return 'Unknown event'
  }
}
export function eventMatches(e: GameEvent, filter: string) {
  if (filter === 'all') return true
  if (filter === 'combat_resolved')
    return (
      ['combat_resolved', 'dice_rolled', 'unit_removed'].includes(e.kind) ||
      (e.kind === 'decision_opened' &&
        e.decision.kind.startsWith('cna.combat.'))
    )
  if (filter === 'unit_moved')
    return e.kind === 'unit_moved' || e.kind === 'note'
  return e.kind === filter
}
