import { describe, expect, it } from 'vitest'
import { decodeObservation, observedWait, receivedRate } from './monitoring'
import { initialState, receive } from './stream/model'
import type { Clock, ServerMessage } from './protocol'
const clock: Clock = {
  game_turn: 1,
  op_stage: 1,
  date: '',
  stage: 'opstage',
  phase: 'movement',
  segment: null,
  step: null,
  phasing: 'axis',
}
const pending = {
  id: 'opaque',
  seat: 'axis.commander',
  kind: 'orders',
  summary: 'Choose',
  opened_seq: 1,
  rules: [],
}
function connected() {
  let s = receive(
    initialState(),
    {
      type: 'hello',
      protocol: 1,
      perspective: 'operator',
      campaign: {
        id: 'test',
        title: 'Test',
        scenario_id: 'test',
        rules_profile: 'test',
        seats: [],
      },
    },
    0,
  ).state
  s = receive(
    s,
    {
      type: 'snapshot',
      seq: 1,
      view: { clock, units: {}, stacks: [], markers: [], pending: [pending] },
    },
    1000,
  ).state
  return s
}
const resolved: ServerMessage = {
  type: 'event',
  seq: 2,
  clock,
  event: {
    kind: 'decision_resolved',
    decision_id: pending.id,
    seat: pending.seat,
    summary: 'Accepted',
    explanation: '<script>literal</script>',
  },
}
describe('operator monitoring', () => {
  it('retains last acceptance and counts beyond ring eviction, clears on gaps and perspective changes', () => {
    let s = receive(connected(), resolved, 11000).state
    expect(s.monitoring.answered).toBe(1)
    expect(s.monitoring.pendingSince).toEqual({})
    expect(receivedRate(s.monitoring, 11000)).toBe(6)
    for (let seq = 3; seq < 650; seq++)
      s = receive(
        s,
        { type: 'event', seq, clock, event: { kind: 'note', text: 'Traffic' } },
        12000,
      ).state
    expect(s.frames.some((f) => f.seq === 2)).toBe(false)
    expect(s.monitoring.seats[pending.seat].explanation).toBe(
      '<script>literal</script>',
    )
    expect(receivedRate(s.monitoring, 80000)).toBe(0)
    expect(
      receive(s, { ...resolved, seq: 900 }, 90000).state.monitoring.answered,
    ).toBe(0)
    expect(initialState('side:commonwealth').monitoring.seats).toEqual({})
  })
  it('deduplicates transcript replay and prevents stale accepted summaries overwriting newer ones', () => {
    let s = receive(connected(), resolved, 11000).state
    const transcript: ServerMessage = {
      type: 'transcript',
      seat: pending.seat,
      tseq: 1,
      game_seq: 2,
      at: '2026-10-07T00:00:00Z',
      entry: {
        kind: 'decision_submitted',
        decision_id: pending.id,
        summary: 'Accepted transcript',
      },
    }
    s = receive(s, transcript, 12000).state
    expect(s.monitoring.seats[pending.seat].summary).toBe('Accepted transcript')
    expect(s.monitoring.seats[pending.seat].explanation).toBe(
      '<script>literal</script>',
    )
    expect(receive(s, transcript, 13000).state).toBe(s)
    s = receive(
      s,
      {
        ...transcript,
        tseq: 2,
        game_seq: 1,
        entry: {
          kind: 'decision_submitted',
          decision_id: 'older',
          summary: 'Old',
        },
      },
      13000,
    ).state
    expect(s.monitoring.seats[pending.seat].summary).toBe('Accepted transcript')
    expect(s.monitoring.answered).toBe(1)
  })
  it('reports observed waiting rather than inventing opening time and rejects malformed bindings', () => {
    expect(observedWait(1000, 65000)).toBe('1m 4s observed')
    expect(observedWait(undefined, 1000)).toBe('duration not reported')
    expect(receivedRate(connected().monitoring, 2000)).toBeNull()
    expect(
      decodeObservation({
        paused: true,
        failure: 'Stopped',
        controller_epoch: 3,
      }),
    ).toEqual({ paused: true, failure: 'Stopped', epoch: 3 })
    expect(() =>
      decodeObservation({ paused: true, failure: null, controller_epoch: -1 }),
    ).toThrow()
  })
})
