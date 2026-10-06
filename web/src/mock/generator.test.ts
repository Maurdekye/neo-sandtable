import { expect, it } from 'vitest'
import { project } from './generator'
import type { ViewState } from '../protocol'
it('projects the fixture upstream without leaking enemy composition or pending decisions', () => {
  const view: ViewState = {
    clock: {
      game_turn: 1,
      date: '1940-09-15',
      stage: 'opstage',
      op_stage: 1,
      phase: 'movement',
      segment: null,
      step: null,
      phasing: null,
    },
    stacks: [
      {
        hex: 'C4218',
        side: 'commonwealth',
        unit_ids: ['secret'],
        visible_count: 1,
      },
    ],
    units: {
      secret: {
        id: 'secret',
        side: 'commonwealth',
        name: 'Secret',
        kind: 'armor',
        size: 'division',
        nationality: 'british',
        hex: 'C4218',
        parent: null,
        detail: { strength: 99 },
      },
    },
    markers: [],
    pending: [
      {
        id: 'd',
        seat: 'commonwealth.front_line',
        kind: 'move',
        summary: 'Secret',
        opened_seq: 1,
        rules: [],
      },
    ],
  }
  const axis = project(view, 'side:axis')
  expect(axis.units).toEqual({})
  expect(axis.pending).toEqual([])
  expect(axis.stacks[0]).toEqual({
    hex: 'C4218',
    side: 'commonwealth',
    unit_ids: [],
    visible_count: null,
  })
  expect(project(view, 'operator')).toBe(view)
})
