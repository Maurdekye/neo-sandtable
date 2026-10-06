import { describe, expect, it } from 'vitest'
import type {
  CampaignMeta,
  Clock,
  GameEvent,
  ServerMessage,
  ViewState,
} from '../protocol'
import {
  advance,
  applyEvent,
  initialState,
  MAX_FRAMES,
  MAX_TRANSCRIPTS,
  receive,
  seek,
  selectedFrame,
} from './model'
const clock: Clock = {
  game_turn: 1,
  date: '1940-09-15',
  stage: 'opstage',
  op_stage: 1,
  phase: 'movement_and_combat',
  segment: 'movement',
  step: null,
  phasing: 'axis',
}
const view: ViewState = {
  clock,
  stacks: [{ hex: 'C4218', side: 'axis', unit_ids: ['u'], visible_count: 1 }],
  units: {
    u: {
      id: 'u',
      side: 'axis',
      name: 'Fixture',
      kind: 'infantry',
      size: 'battalion',
      nationality: 'italian',
      hex: 'C4218',
      parent: null,
      detail: null,
    },
  },
  markers: [],
  pending: [],
}
const campaign: CampaignMeta = {
  id: 'test',
  scenario_id: 'fixture',
  rules_profile: 'fixture',
  title: 'Fixture',
  seats: [],
}
function connected() {
  let s = receive(initialState(), {
    type: 'hello',
    protocol: 1,
    campaign,
    perspective: 'operator',
  }).state
  s = receive(s, { type: 'snapshot', seq: 7, view }).state
  return s
}
const event = (seq: number): ServerMessage => ({
  type: 'event',
  seq,
  clock,
  event: { kind: 'note', text: 'Fixture' },
})
describe('stream and replay invariants', () => {
  it('requires hello and snapshot before events', () => {
    expect(receive(initialState(), event(1)).state.lastSeq).toBeNull()
    expect(
      receive(initialState(), { type: 'snapshot', seq: 0, view }).state.frames,
    ).toHaveLength(0)
    expect(connected().connection).toBe('live')
  })
  it('drops stale state on a gap and requests a fresh snapshot', () => {
    const s = receive(connected(), event(8)).state
    const gap = receive(s, event(10))
    expect(gap.state.frames).toEqual([])
    expect(gap.state.connection).toBe('resyncing')
    expect(gap.subscribe).toEqual({
      type: 'subscribe',
      perspective: 'operator',
      from_seq: null,
    })
  })
  it('ignores unknown kinds while advancing the contiguous sequence', () => {
    const e = { kind: 'future_payload', value: 42 } as unknown as GameEvent
    const s = receive(connected(), {
      type: 'event',
      seq: 8,
      clock,
      event: e,
    }).state
    expect(s.lastSeq).toBe(8)
    expect(s.frames.at(-1)!.view.units).toBe(view.units)
  })
  it('keeps buffered live events separate from a paused frame and clamps evicted history', () => {
    let s = seek(connected(), 7)
    s = receive(s, event(8)).state
    expect(selectedFrame(s)?.seq).toBe(7)
    expect(s.lastSeq).toBe(8)
    expect(selectedFrame(advance(s))?.seq).toBe(8)
    for (let n = 9; n < MAX_FRAMES + 20; n++) s = receive(s, event(n)).state
    expect(s.frames).toHaveLength(MAX_FRAMES)
    expect(s.cursor).toBe(s.frames[0].seq)
  })
  it('deduplicates per-seat transcripts and bounds transcript memory', () => {
    let s = connected()
    const message = {
      type: 'transcript' as const,
      seat: 'axis.commander',
      tseq: 1,
      at: '2026-10-06T10:00:00Z',
      game_seq: 7,
      entry: { kind: 'assistant_text' as const, text: 'Fixture' },
    }
    s = receive(s, message).state
    s = receive(s, message).state
    expect(s.transcripts).toHaveLength(1)
    for (let tseq = 2; tseq < MAX_TRANSCRIPTS + 10; tseq++)
      s = receive(s, { ...message, tseq }).state
    expect(s.transcripts).toHaveLength(MAX_TRANSCRIPTS)
  })
  it('moves units without changing earlier frames and removes empty stacks', () => {
    const next = applyEvent(view, {
      kind: 'unit_moved',
      unit_id: 'u',
      path: ['C4218', 'C4219'],
      cp_spent: null,
    })
    expect(next.units.u.hex).toBe('C4219')
    expect(next.stacks).toEqual([
      { hex: 'C4219', side: 'axis', unit_ids: ['u'], visible_count: 1 },
    ])
    expect(view.units.u.hex).toBe('C4218')
    expect(view.stacks[0].unit_ids).toEqual(['u'])
  })
  it('replaces and removes enemy stack presence without needing unit detail', () => {
    let v = applyEvent(view, {
      kind: 'stack_updated',
      stack: {
        hex: 'C4219',
        side: 'commonwealth',
        unit_ids: [],
        visible_count: null,
      },
    })
    expect(v.stacks).toHaveLength(2)
    v = applyEvent(v, {
      kind: 'stack_updated',
      stack: {
        hex: 'C4219',
        side: 'commonwealth',
        unit_ids: [],
        visible_count: 2,
      },
    })
    expect(v.stacks).toHaveLength(2)
    expect(v.stacks[1].visible_count).toBe(2)
    v = applyEvent(v, {
      kind: 'stack_removed',
      hex: 'C4219',
      side: 'commonwealth',
    })
    expect(v.stacks).toEqual(view.stacks)
    expect(v.units).toBe(view.units)
  })
  it('does not infer undisclosed units from an enemy move', () => {
    expect(
      applyEvent(view, {
        kind: 'unit_moved',
        unit_id: 'hidden',
        path: ['C4218', 'C4219'],
        cp_spent: null,
      }),
    ).toBe(view)
  })
  it('applies generated decisions, markers and removals', () => {
    const d = {
      id: 'd',
      seat: 'axis.commander',
      kind: 'move',
      summary: 'Fixture',
      opened_seq: 8,
    }
    let v = applyEvent(view, { kind: 'decision_opened', decision: d })
    expect(v.pending).toEqual([d])
    v = applyEvent(v, {
      kind: 'decision_resolved',
      decision_id: 'd',
      seat: d.seat,
      summary: 'Done',
    })
    expect(v.pending).toEqual([])
    v = applyEvent(v, {
      kind: 'marker_placed',
      marker: {
        id: 'm',
        kind: 'supply_dump',
        hex: 'C4218',
        side: 'axis',
        label: null,
      },
    })
    expect(v.markers).toHaveLength(1)
    v = applyEvent(v, { kind: 'marker_removed', marker_id: 'm' })
    expect(v.markers).toEqual([])
    v = applyEvent(v, { kind: 'unit_removed', unit_id: 'u', reason: 'Fixture' })
    expect(v.units).toEqual({})
    expect(v.stacks).toEqual([])
  })
})
