import type { TranscriptMessage } from '../protocol'
export interface TranscriptRow {
  message: TranscriptMessage
  result?: TranscriptMessage
}
export function pairTranscripts(
  messages: TranscriptMessage[],
): TranscriptRow[] {
  const calls = new Map<string, TranscriptMessage>(),
    results = new Map<string, TranscriptMessage>()
  messages.forEach((m) => {
    if (m.entry.kind === 'tool_call') calls.set(m.entry.call_id, m)
    if (m.entry.kind === 'tool_result') results.set(m.entry.call_id, m)
  })
  return messages
    .filter(
      (m) => m.entry.kind !== 'tool_result' || !calls.has(m.entry.call_id),
    )
    .map((m) => ({
      message: m,
      result:
        m.entry.kind === 'tool_call' ? results.get(m.entry.call_id) : undefined,
    }))
}
