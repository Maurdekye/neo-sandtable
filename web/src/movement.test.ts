import { expect, it } from 'vitest'
import {
  motionEvents,
  movedUnits,
  movementUnits,
  routeSampler,
} from './movement'
import type { PendingDecision } from './generated/PendingDecision'
import type { ViewState, Clock } from './protocol'
import type { Frame } from './stream/model'
import { demoHex, center, HEX_BY_ID } from './map/fixture'
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
const hex = demoHex(16, 12),
  target = demoHex(17, 12),
  end = demoHex(17, 13)
const view: ViewState = {
  clock,
  units: {
    a: {
      id: 'a',
      name: 'A',
      side: 'axis',
      kind: 'infantry',
      size: 'battalion',
      nationality: 'italian',
      hex,
      parent: null,
      detail: {},
    },
  },
  stacks: [],
  pending: [],
  markers: [],
}
const frames: Frame[] = [
  { seq: 0, view, event: null },
  {
    seq: 1,
    view: { ...view, units: { a: { ...view.units.a, hex: end } } },
    event: {
      kind: 'unit_moved',
      unit_id: 'a',
      path: [target, end],
      cp_spent: null,
    },
  },
]
it('uses the exact published movement action enum, including pass alternatives', () => {
  const decision: PendingDecision = {
    id: 'd',
    seat: 'axis.front_line',
    kind: 'cna.movement.orders',
    summary: 'Move',
    rules: ['land:8.11'],
    opened_seq: 0,
    space: {
      anyOf: [
        {
          type: 'array',
          items: {
            type: 'object',
            properties: { unit: { enum: ['a', 'b', 'a'] } },
          },
        },
        { type: 'null' },
      ],
    },
  }
  expect(movementUnits(decision)).toEqual(['a', 'b'])
  expect(movementUnits({ ...decision, kind: 'other' })).toBeNull()
  expect(
    movementUnits({
      ...decision,
      space: {
        type: 'array',
        items: { properties: { unit: { enum: ['a', 42] } } },
      },
    }),
  ).toBeNull()
})
it('animates authorized paths with the prior disclosed origin and never invents an anonymous stack route', () => {
  const motions = motionEvents(frames, 0, 1, true)
  expect(motions[0].points).toEqual(
    [hex, target, end].map((id) => center(HEX_BY_ID.get(id)!)),
  )
  const hidden: Frame[] = [
    { seq: 0, view: { ...view, units: {} }, event: null },
    { ...frames[1], view: { ...view, units: {} } },
  ]
  expect(motionEvents(hidden, 0, 1, true)).toEqual([])
  const stack: Frame = {
    seq: 2,
    view,
    event: {
      kind: 'stack_updated',
      stack: {
        hex: end,
        side: 'commonwealth',
        unit_ids: [],
        visible_count: null,
      },
    },
  }
  expect(motionEvents([...frames, stack], 1, 2, true)).toEqual([
    { key: `stack:${end}:commonwealth`, points: [center(HEX_BY_ID.get(end)!)] },
  ])
  expect(motionEvents(frames, 1, 0, true)).toEqual([])
  expect(motionEvents([...frames, stack], 0, 2, false)).toEqual([])
  expect(motionEvents(frames, null, 1, true)).toEqual([])
})
it('keeps snapshot movement flags and clears event-derived highlights for a different segment or hidden unit', () => {
  expect(movedUnits(frames[1].view, frames, 1).has('a')).toBe(true)
  expect(
    movedUnits({ ...view, clock: { ...clock, segment: 'combat' } }, frames, 1)
      .size,
  ).toBe(0)
  expect(movedUnits({ ...view, units: {} }, frames, 1).size).toBe(0)
  expect(
    movedUnits(
      {
        ...view,
        units: { a: { ...view.units.a, detail: { moved_this_segment: true } } },
      },
      [],
      50,
    ).has('a'),
  ).toBe(true)
})
it('samples multiple unequal path legs by length with clamped progress and repeated points', () => {
  const sample = routeSampler([
    { x: 0, y: 0 },
    { x: 10, y: 0 },
    { x: 10, y: 30 },
  ])
  expect(sample(0.5)).toEqual({ x: 10, y: 10 })
  expect(sample(-1)).toEqual({ x: 0, y: 0 })
  expect(sample(2)).toEqual({ x: 10, y: 30 })
  expect(
    routeSampler([
      { x: 1, y: 1 },
      { x: 1, y: 1 },
    ])(0.5),
  ).toEqual({ x: 1, y: 1 })
})

it('does not carry a snapshot flag into a later segment or an earlier repeated movement cycle', () => {
  const flagged = {
    ...view,
    units: { a: { ...view.units.a, detail: { moved_this_segment: true } } },
  }
  const combat = { ...flagged, clock: { ...clock, segment: 'combat' } }
  const fresh = { ...flagged, clock }
  const history: Frame[] = [
    { seq: 0, view: flagged, event: null },
    {
      seq: 1,
      view: combat,
      event: { kind: 'phase_changed', clock: combat.clock },
    },
    {
      seq: 2,
      view: fresh,
      event: { kind: 'phase_changed', clock: fresh.clock },
    },
  ]
  expect(movedUnits(combat, history, 1).size).toBe(0)
  expect(movedUnits(fresh, history, 2).size).toBe(0)
  const cleared = { ...view.units.a, detail: { moved_this_segment: false } }
  expect(
    movedUnits(
      { ...view, units: { a: cleared } },
      [
        ...frames,
        { seq: 2, view, event: { kind: 'unit_updated', unit: cleared } },
      ],
      2,
    ).size,
  ).toBe(0)
})

it('optional nullable record fields do not alter the required movement unit enum', () => {
  const d: PendingDecision = {
    id: 'nullable',
    seat: 'axis.front_line',
    kind: 'cna.movement.orders',
    summary: 'Move',
    opened_seq: 0,
    rules: [],
    space: {
      type: 'array',
      items: {
        type: 'object',
        required: ['unit', 'path'],
        properties: {
          unit: { enum: ['a', 'b'] },
          path: { type: 'array', items: { type: 'string' } },
          detach: { anyOf: [{ type: 'boolean' }, { type: 'null' }] },
          note: { anyOf: [{ type: 'string' }, { type: 'null' }] },
        },
      },
    },
  }
  expect(movementUnits(d)).toEqual(['a', 'b'])
})
