import type { GameEvent, TranscriptMessage } from './protocol'
import type { Frame } from './stream/model'
export const MAX_COMMENTARIES = 256
export interface AcceptedCommentary {
  seat: string
  decisionId: string
  text: string
  frame: Frame
}
/** Only accepted canonical events qualify; model prose and submit tool arguments do not. */
export function explanation(event: GameEvent | null): string | undefined {
  if (
    event?.kind !== 'decision_resolved' ||
    !('explanation' in event) ||
    typeof event.explanation !== 'string'
  )
    return undefined
  return event.explanation.trim() || undefined
}
export function recordCommentary(
  entries: AcceptedCommentary[],
  frame: Frame,
): AcceptedCommentary[] {
  const text = explanation(frame.event),
    e = frame.event
  if (!text || e?.kind !== 'decision_resolved') return entries
  return [
    ...entries,
    { seat: e.seat, decisionId: e.decision_id, text, frame },
  ].slice(-MAX_COMMENTARIES)
}
export function decisionCommentary(
  entries: AcceptedCommentary[],
  m: TranscriptMessage,
): AcceptedCommentary | undefined {
  if (m.entry.kind !== 'decision_submitted') return undefined
  const id = m.entry.decision_id
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (e.seat === m.seat && e.decisionId === id && e.frame.seq <= m.game_seq)
      return e
  }
  return undefined
}
