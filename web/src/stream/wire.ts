import type { ServerMessage } from '../protocol'
function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
function sequence(value: unknown) {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
}
function clock(value: unknown) {
  return (
    object(value) &&
    typeof value.game_turn === 'number' &&
    typeof value.date === 'string' &&
    typeof value.stage === 'string' &&
    typeof value.phase === 'string'
  )
}
function stack(value: unknown) {
  return (
    object(value) &&
    typeof value.hex === 'string' &&
    (value.side === 'axis' || value.side === 'commonwealth') &&
    Array.isArray(value.unit_ids) &&
    value.unit_ids.every((id) => typeof id === 'string') &&
    (value.visible_count === null || typeof value.visible_count === 'number')
  )
}
function unit(value: unknown) {
  return (
    object(value) &&
    ['id', 'side', 'name', 'kind', 'size', 'nationality'].every(
      (key) => typeof value[key] === 'string',
    ) &&
    (value.hex === null || typeof value.hex === 'string')
  )
}
function marker(value: unknown) {
  return (
    object(value) &&
    ['id', 'kind', 'hex'].every((key) => typeof value[key] === 'string') &&
    (value.label === null || typeof value.label === 'string')
  )
}
function decision(value: unknown) {
  return (
    object(value) &&
    ['id', 'seat', 'kind', 'summary'].every(
      (key) => typeof value[key] === 'string',
    ) &&
    sequence(value.opened_seq)
  )
}
function gameEvent(value: unknown) {
  if (!object(value) || typeof value.kind !== 'string') return false
  switch (value.kind) {
    case 'unit_moved':
      return (
        typeof value.unit_id === 'string' &&
        Array.isArray(value.path) &&
        value.path.every((id) => typeof id === 'string')
      )
    case 'unit_updated':
      return unit(value.unit)
    case 'unit_removed':
      return (
        typeof value.unit_id === 'string' && typeof value.reason === 'string'
      )
    case 'stack_updated':
      return stack(value.stack)
    case 'stack_removed':
      return typeof value.hex === 'string' && typeof value.side === 'string'
    case 'phase_changed':
      return clock(value.clock)
    case 'marker_placed':
      return marker(value.marker)
    case 'marker_removed':
      return typeof value.marker_id === 'string'
    case 'decision_opened':
      return decision(value.decision)
    case 'decision_resolved':
      return (
        typeof value.decision_id === 'string' &&
        typeof value.summary === 'string'
      )
    case 'combat_resolved':
      return typeof value.hex === 'string' && typeof value.summary === 'string'
    case 'dice_rolled':
      return (
        typeof value.purpose === 'string' &&
        Array.isArray(value.dice) &&
        value.dice.every((d) => typeof d === 'number')
      )
    case 'note':
      return typeof value.text === 'string'
    default:
      return true // Unknown game kinds still advance the perspective cursor.
  }
}
function transcript(value: unknown) {
  if (!object(value)) return false
  switch (value.kind) {
    case 'assistant_text':
    case 'reasoning':
    case 'system':
      return typeof value.text === 'string'
    case 'tool_call':
      return (
        typeof value.call_id === 'string' &&
        typeof value.tool === 'string' &&
        'args' in value
      )
    case 'tool_result':
      return (
        typeof value.call_id === 'string' &&
        typeof value.ok === 'boolean' &&
        typeof value.summary === 'string'
      )
    case 'decision_submitted':
      return (
        typeof value.decision_id === 'string' &&
        typeof value.summary === 'string'
      )
    case 'system1_query':
      return (
        typeof value.question === 'string' &&
        Array.isArray(value.options) &&
        value.options.every((o) => typeof o === 'string')
      )
    case 'system1_answer':
      return typeof value.choice === 'string'
    default:
      return false
  }
}
/** Guard the JSON boundary; generated protocol types remain the contract. */
export function decodeMessage(data: unknown): ServerMessage | null {
  if (typeof data !== 'string') throw new Error('Expected a JSON text stream')
  const m: unknown = JSON.parse(data)
  if (!object(m) || typeof m.type !== 'string')
    throw new Error('Invalid stream envelope')
  let valid = false
  switch (m.type) {
    case 'hello':
      valid =
        m.protocol === 1 &&
        typeof m.perspective === 'string' &&
        object(m.campaign) &&
        ['id', 'scenario_id', 'rules_profile', 'title'].every(
          (key) =>
            typeof (m.campaign as Record<string, unknown>)[key] === 'string',
        ) &&
        Array.isArray(m.campaign.seats) &&
        m.campaign.seats.every(
          (s) =>
            object(s) &&
            typeof s.id === 'string' &&
            typeof s.side === 'string' &&
            (s.controller === null ||
              (object(s.controller) && typeof s.controller.label === 'string')),
        )
      break
    case 'snapshot':
      valid =
        sequence(m.seq) &&
        object(m.view) &&
        clock(m.view.clock) &&
        Array.isArray(m.view.stacks) &&
        m.view.stacks.every(stack) &&
        object(m.view.units) &&
        Object.values(m.view.units).every(unit) &&
        Array.isArray(m.view.markers) &&
        m.view.markers.every(marker) &&
        Array.isArray(m.view.pending) &&
        m.view.pending.every(decision)
      break
    case 'event':
      valid = sequence(m.seq) && clock(m.clock) && gameEvent(m.event)
      break
    case 'transcript':
      valid =
        typeof m.seat === 'string' &&
        sequence(m.tseq) &&
        sequence(m.game_seq) &&
        typeof m.at === 'string' &&
        transcript(m.entry)
      break
    case 'resync':
      valid = true
      break
    default:
      return null
  }
  if (!valid) throw new Error(`Invalid ${m.type} stream payload`)
  return m as unknown as ServerMessage
}
