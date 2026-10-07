import { expect, it } from 'vitest'
import {
  recordStage,
  summaryFrame,
  MAX_STAGES,
  MAX_STAGE_ENTRIES,
} from './stageTimeline'
import type { Frame } from './stream/model'
import type { Clock, GameEvent, ViewState } from './protocol'
const clock: Clock = {
  game_turn: 1,
  date: '1940-09-15',
  op_stage: 1,
  stage: 'opstage',
  phase: 'movement_and_combat',
  segment: 'movement',
  step: null,
  phasing: 'axis',
}
const view: ViewState = {
  clock,
  units: {
    a: {
      id: 'a',
      name: 'Own A',
      side: 'axis',
      kind: 'infantry',
      size: 'battalion',
      nationality: 'italian',
      hex: 'C4120',
      parent: 'division',
      detail: null,
    },
  },
  stacks: [],
  markers: [],
  pending: [],
}
const frame = (seq: number, event: GameEvent | null, hex?: string): Frame => ({
  seq,
  event,
  view,
  hex,
})
it('counts received moves per declared formation and retains the largest exact route checkpoint', () => {
  let s = recordStage([], frame(0, null))
  const first = frame(1, {
    kind: 'unit_moved',
    unit_id: 'a',
    path: ['C4020', 'C3921'],
    cp_spent: 3,
  })
  s = recordStage(s, first, frame(0, null))
  s = recordStage(
    s,
    frame(2, {
      kind: 'unit_moved',
      unit_id: 'a',
      path: ['C4220'],
      cp_spent: 1,
    }),
    first,
  )
  expect(s[0].moves).toBe(2)
  expect(s[0].formations[0]).toMatchObject({
    id: 'division',
    count: 2,
    longest: 2,
  })
  expect(summaryFrame(s, 1)).toBe(first)
})
it('keeps prose unlocated and labels supply notes without inferring shortage totals', () => {
  const note = frame(1, { kind: 'note', text: 'Fuel shortage note at C4120' })
  const s = recordStage([], note)
  expect(s[0].entries[0]).toMatchObject({
    category: 'supply',
    hex: null,
    label: 'Fuel shortage note at C4120',
  })
  expect(s[0].counts.supply).toBe(1)
})
it('keeps a legacy stack update anonymous when faces and count are absent', () => {
  const f = frame(1, {
    kind: 'stack_updated',
    stack: {
      hex: 'C4220',
      side: 'commonwealth',
      visible_count: null,
      unit_ids: [],
    },
  })
  const s = recordStage([], { ...f, view: { ...view, units: {} } })
  expect(s[0].moves).toBe(0)
  expect(s[0].formations).toEqual([])
  expect(s[0].entries[0].label).toBe('commonwealth stack updated at C4220')
})
it('bounds history and entry details but keeps received counts cumulative', () => {
  let s = recordStage([], frame(5, null))
  for (let n = 1; n <= MAX_STAGE_ENTRIES + 10; n++)
    s = recordStage(s, frame(5 + n, { kind: 'note', text: 'received note' }))
  expect(s[0].partial).toBe(true)
  expect(s[0].entries).toHaveLength(MAX_STAGE_ENTRIES)
  expect(s[0].counts.notes).toBe(MAX_STAGE_ENTRIES + 10)
  expect(s[0].droppedEntries).toBe(10)
  for (let n = 2; n <= MAX_STAGES + 2; n++)
    s = recordStage(s, {
      ...frame(n + 100, null),
      view: { ...view, clock: { ...clock, op_stage: n } },
    })
  expect(s).toHaveLength(MAX_STAGES)
  expect(s[0].clock.op_stage).toBe(3)
})

it('groups received enemy movement by its counter and distinguishes visibility from losses', () => {
  const unit = {
    ...view.units.a,
    id: 'enemy',
    side: 'commonwealth' as const,
    name: 'Enemy face',
    parent: null,
    detail: { counter: 'E', stacking_points: 2 },
  }
  const appeared = {
    ...frame(1, { kind: 'unit_updated', unit }),
    view: { ...view, units: { enemy: unit } },
  }
  let s = recordStage([], appeared, {
    ...frame(0, null),
    view: { ...view, units: {} },
  })
  s = recordStage(
    s,
    {
      ...frame(2, {
        kind: 'unit_moved',
        unit_id: 'enemy',
        path: ['C4220'],
        cp_spent: null,
      }),
      view: appeared.view,
    },
    appeared,
  )
  s = recordStage(
    s,
    frame(3, { kind: 'unit_removed', unit_id: 'enemy', reason: '' }),
  )
  expect(s[0].formations[0]).toMatchObject({
    id: 'enemy',
    label: 'Enemy face',
    count: 1,
  })
  expect(s[0].counts.counters).toBe(2)
  expect(s[0].counts.removals).toBeUndefined()
  expect(s[0].entries.some((e) => e.label.includes('no longer visible'))).toBe(
    true,
  )
  s = recordStage(
    s,
    frame(4, { kind: 'unit_removed', unit_id: 'enemy', reason: 'combat loss' }),
  )
  expect(s[0].counts.removals).toBe(1)
})
