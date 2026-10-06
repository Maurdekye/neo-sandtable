import { expect, it } from 'vitest'
import type { TranscriptMessage } from '../protocol'
import { pairTranscripts } from './transcripts'
it('pairs tool calls with out-of-order results and preserves orphan results', () => {
  const base = {
    type: 'transcript' as const,
    seat: 'axis.commander',
    at: '2026-10-06T10:00:00Z',
    game_seq: 1,
  }
  const messages: TranscriptMessage[] = [
    {
      ...base,
      tseq: 1,
      entry: {
        kind: 'tool_result',
        call_id: 'c',
        ok: true,
        summary: 'Done',
        detail: null,
      },
    },
    {
      ...base,
      tseq: 2,
      entry: { kind: 'tool_call', call_id: 'c', tool: 'observe', args: {} },
    },
    {
      ...base,
      tseq: 3,
      entry: {
        kind: 'tool_result',
        call_id: 'lost',
        ok: false,
        summary: 'Call evicted',
        detail: null,
      },
    },
  ]
  const rows = pairTranscripts(messages)
  expect(rows).toHaveLength(2)
  expect(rows[0].result?.entry.kind).toBe('tool_result')
  expect(rows[1].message.entry.kind).toBe('tool_result')
})
