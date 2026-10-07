import { expect, it } from 'vitest'
import {
  decisionCommentary,
  recordCommentary,
  MAX_COMMENTARIES,
} from './commentary'
import type { Frame } from './stream/model'
import type { GameEvent, TranscriptMessage } from './protocol'
const text = '<img src=x onerror=alert(1)> https://example.test **literal**'
const frame: Frame = {
  seq: 0,
  event: null,
  view: {
    clock: {
      game_turn: 1,
      date: '1940-09-15',
      op_stage: 1,
      stage: 'opstage',
      phase: 'movement_and_combat',
      segment: 'movement',
      step: null,
      phasing: 'axis',
    },
    units: {},
    stacks: [],
    markers: [],
    pending: [],
  },
}
const event = JSON.parse(
  JSON.stringify({
    kind: 'decision_resolved',
    seat: 'axis.front_line',
    decision_id: 'opaque',
    summary: 'Accepted',
    explanation: text,
  }),
) as GameEvent
const message: TranscriptMessage = {
  type: 'transcript',
  seat: 'axis.front_line',
  tseq: 1,
  game_seq: 5,
  at: '1940-09-15T00:00:00Z',
  entry: {
    kind: 'decision_submitted',
    decision_id: 'opaque',
    summary: 'Action',
  },
}
it('pairs only canonical accepted explanation with its own seat and decision after its game sequence', () => {
  const entries = recordCommentary([], { ...frame, seq: 5, event })
  expect(decisionCommentary(entries, message)?.text).toBe(text)
  expect(
    decisionCommentary(entries, {
      ...message,
      seat: 'commonwealth.front_line',
    }),
  ).toBeUndefined()
  expect(
    decisionCommentary(entries, { ...message, game_seq: 4 }),
  ).toBeUndefined()
  expect(
    recordCommentary([], {
      ...frame,
      event: {
        kind: 'decision_resolved',
        seat: 'axis.front_line',
        decision_id: 'opaque',
        summary: 'Baseline',
      },
    }),
  ).toEqual([])
  expect(
    recordCommentary([], {
      ...frame,
      event: { kind: 'note', text: 'Invented commentary' },
    }),
  ).toEqual([])
})
it('bounds accepted commentary checkpoints', () => {
  let entries = recordCommentary([], { ...frame, seq: 1, event })
  for (let seq = 2; seq <= MAX_COMMENTARIES + 2; seq++)
    entries = recordCommentary(entries, { ...frame, seq, event })
  expect(entries).toHaveLength(MAX_COMMENTARIES)
  expect(entries[0].frame.seq).toBe(3)
})
