import type { UsageSnapshot } from './generated/UsageSnapshot'
import type { ServerMessage } from './protocol'
import type { PendingDecision } from './generated/PendingDecision'
export interface MonitorState {
  started: number | null
  answered: number
  recent: number[]
  usage: Record<string, UsageSnapshot>
  pendingSince: Record<string, number>
  seats: Record<
    string,
    {
      summary?: string
      explanation?: string
      decisionId?: string
      seq?: number
      activity?: string
      toolError?: string
    }
  >
}
export const emptyMonitor = (): MonitorState => ({
  started: null,
  answered: 0,
  recent: [],
  usage: {},
  pendingSince: {},
  seats: {},
})
export function startMonitor(
  pending: PendingDecision[],
  now: number,
): MonitorState {
  return {
    ...emptyMonitor(),
    started: now,
    pendingSince: Object.fromEntries(
      pending.slice(-512).map((d) => [d.id, now]),
    ),
  }
}
// Called only after stream sequencing/deduplication succeeds. Keep aggregate counts beyond frame eviction.
export function recordMonitor(
  old: MonitorState,
  message: ServerMessage,
  now: number,
): MonitorState {
  const next = { ...old, recent: old.recent.filter((t) => t > now - 60000) }
  if (message.type === 'event') {
    const e = message.event
    if (e.kind === 'decision_opened')
      next.pendingSince = { ...old.pendingSince, [e.decision.id]: now }
    if (e.kind === 'decision_resolved') {
      next.answered++
      next.recent = [...next.recent, now].slice(-10000)
      next.pendingSince = { ...old.pendingSince }
      delete next.pendingSince[e.decision_id]
      next.seats = {
        ...old.seats,
        [e.seat]: {
          ...old.seats[e.seat],
          summary: e.summary,
          explanation: e.explanation,
          decisionId: e.decision_id,
          seq: message.seq,
        },
      }
    }
  } else if (message.type === 'transcript') {
    const e = message.entry
    if (e.kind === 'usage_snapshot') {
      const prior = old.usage[message.seat]
      if (
        !prior ||
        e.controller_epoch > prior.controller_epoch ||
        (e.controller_epoch === prior.controller_epoch &&
          e.revision > prior.revision)
      )
        next.usage = Object.fromEntries(
          Object.entries({ ...old.usage, [message.seat]: e }).slice(-128),
        )
    }
    const activity = e.kind.replaceAll('_', ' ')
    const seat = { ...old.seats[message.seat], activity }
    if (
      e.kind === 'decision_submitted' &&
      (seat.seq === undefined || message.game_seq >= seat.seq)
    ) {
      seat.summary = e.summary
      if (seat.decisionId !== e.decision_id) seat.explanation = undefined
      seat.decisionId = e.decision_id
      seat.seq = message.game_seq
    }
    if (e.kind === 'tool_result') seat.toolError = e.ok ? undefined : e.summary
    next.seats = { ...old.seats, [message.seat]: seat }
  }
  // The protocol's campaign has ten seats; bound malformed or future over-sized streams too.
  next.seats = Object.fromEntries(Object.entries(next.seats).slice(-128))
  next.pendingSince = Object.fromEntries(
    Object.entries(next.pendingSince).slice(-512),
  )
  return next
}
export function receivedRate(state: MonitorState, now: number) {
  if (state.started === null) return null
  const seconds = Math.min(60, Math.max(0, (now - state.started) / 1000))
  // Avoid presenting a burst during attachment as a meaningful measured rate.
  return seconds >= 10
    ? (state.recent.filter((t) => t > now - 60000).length * 60) / seconds
    : null
}
export function observedWait(since: number | undefined, now: number): string {
  if (since === undefined) return 'duration not reported'
  const seconds = Math.max(0, Math.floor((now - since) / 1000))
  return seconds < 60
    ? `${seconds}s observed`
    : `${Math.floor(seconds / 60)}m ${seconds % 60}s observed`
}
export interface SeatObservation {
  paused: boolean
  failure: string | null
  epoch: number
}
export function decodeObservation(value: unknown): SeatObservation {
  if (typeof value !== 'object' || value === null)
    throw new Error('Invalid seat status')
  const v = value as Record<string, unknown>
  if (
    typeof v.paused !== 'boolean' ||
    !(v.failure === null || typeof v.failure === 'string') ||
    typeof v.controller_epoch !== 'number' ||
    !Number.isSafeInteger(v.controller_epoch) ||
    v.controller_epoch < 0
  )
    throw new Error('Invalid seat status')
  return { paused: v.paused, failure: v.failure, epoch: v.controller_epoch }
}

export function reportedNumber(value: number | null | undefined): string {
  return value === null || value === undefined
    ? 'not reported'
    : value.toLocaleString('en-US', { maximumFractionDigits: 6 })
}
export function usageLabel(usage: UsageSnapshot | undefined): string {
  return usage
    ? `Input ${reportedNumber(usage.input_tokens)} / output ${reportedNumber(usage.output_tokens)} / USD ${usage.reported_cost_usd === null ? 'not reported' : '$' + reportedNumber(usage.reported_cost_usd)}${usage.incomplete_turns ? ' / incomplete' : ''}`
    : 'Tokens: not reported / USD: not reported'
}
