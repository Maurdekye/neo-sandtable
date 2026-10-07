import { expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { createElement } from 'react'
import {
  placementContext,
  placementDestinations,
  placementHexes,
} from './setup'
import { PendingDecisions } from './PendingDecisions'
import { motionEvents } from './movement'
import type { PendingDecision } from './generated/PendingDecision'
import type { Frame } from './stream/model'
import type { ViewState } from './protocol'
import { demoHex } from './map/fixture'
const hex = demoHex(16, 12)
const request: PendingDecision = {
  id: 'opaque-choice',
  seat: 'axis.commander',
  kind: 'cna.setup.unit',
  summary: 'Place unit',
  opened_seq: 1,
  rules: ['scen:60.31'],
  space: { type: 'string', enum: [hex, hex, 'box_tripoli'] },
}
it('keeps off-map choices separate and refuses to infer missing or malformed legal domains', () => {
  expect(placementDestinations(request)).toEqual([hex, 'box_tripoli'])
  expect(placementHexes(request)).toEqual([hex])
  expect(placementHexes(undefined)).toEqual([])
  expect(placementDestinations({ ...request, space: undefined })).toBeNull()
  expect(
    placementDestinations({
      ...request,
      space: { type: 'string', enum: [hex, 7] },
    }),
  ).toBeNull()
  expect(
    placementDestinations({ ...request, kind: 'cna.setup.first_line_trucks' }),
  ).toBeNull()
})
it('removes private placement choices when the authorized pending projection is empty', () => {
  const props = {
    units: {},
    onUnit: () => {},
    onHex: () => {},
    onPlacement: () => {},
    activePlacement: request.id,
  }
  const own = renderToStaticMarkup(
    createElement(PendingDecisions, { ...props, pending: [request] }),
  )
  const other = renderToStaticMarkup(
    createElement(PendingDecisions, { ...props, pending: [] }),
  )
  expect(own).toContain('Highlighted legal area')
  expect(own).toContain('box_tripoli')
  expect(other).not.toContain('setup-destinations')
  expect(other).not.toContain('opaque-choice')
})
it('animates a disclosed awaiting-unit transition only after its map position is published', () => {
  const unit = {
    id: 'a',
    side: 'axis' as const,
    name: 'A',
    hex: null,
    kind: 'infantry',
    size: 'battalion',
    nationality: 'italian',
    parent: null,
    detail: { location: { at: 'awaiting_setup', group: 'g' } },
  }
  const view: ViewState = {
    units: { a: unit },
    stacks: [],
    markers: [],
    pending: [request],
    clock: {
      game_turn: 1,
      date: '',
      op_stage: 0,
      stage: 'setup',
      phase: '',
      segment: null,
      step: null,
      phasing: 'axis',
    },
  }
  const placed = { ...unit, hex, detail: {} }
  const frames: Frame[] = [
    { seq: 0, event: null, view },
    {
      seq: 1,
      event: { kind: 'unit_updated', unit: placed },
      view: { ...view, units: { a: placed }, pending: [] },
    },
  ]
  expect(motionEvents(frames, 0, 1, true)).toMatchObject([
    { unitId: 'a', points: [expect.any(Object)] },
  ])
  const privateUpdate: Frame = {
    seq: 1,
    event: { kind: 'unit_updated', unit },
    view,
  }
  expect(motionEvents([frames[0], privateUpdate], 0, 1, true)).toEqual([])
  expect(motionEvents(frames, null, 1, true)).toEqual([])
  expect(motionEvents(frames, 1, 0, true)).toEqual([])
})

it('reads structured context and refuses guessed ids from summary or malformed values', () => {
  expect(
    placementContext({
      ...request,
      space: {
        type: 'string',
        enum: [hex],
        'x-context': { unit: 'unit-a', group: 'group-a' },
      },
    }),
  ).toEqual({ unit: 'unit-a', group: 'group-a' })
  expect(placementContext(request)).toEqual({})
  expect(
    placementContext({
      ...request,
      summary: 'Place unit-a within its scenario setup area.',
    }),
  ).toEqual({})
  expect(
    placementContext({
      ...request,
      space: { 'x-context': { unit: 7, group: ['g'] } },
    }),
  ).toEqual({})
})

it('links a disclosed awaiting unit by x-context while older projections show an explicit fallback', () => {
  const unit = {
    id: 'a',
    name: 'Awaiting battalion',
    side: 'axis' as const,
    hex: null,
    kind: 'infantry',
    size: 'battalion',
    nationality: 'italian',
    parent: null,
    detail: { location: { at: 'awaiting_setup', group: 'g' } },
  }
  const props = {
    units: { a: unit },
    onUnit: () => {},
    onHex: () => {},
    onPlacement: () => {},
    activePlacement: request.id,
  }
  const structured = {
    ...request,
    space: {
      type: 'string',
      enum: [hex],
      'x-context': { unit: 'a', group: 'g' },
    },
  }
  const own = renderToStaticMarkup(
    createElement(PendingDecisions, { ...props, pending: [structured] }),
  )
  expect(own).toContain('data-unit-id="a"')
  expect(own).toContain('Awaiting battalion')
  expect(own).toContain('Placement decision open')
  const absent = renderToStaticMarkup(
    createElement(PendingDecisions, { ...props, pending: [request] }),
  )
  expect(absent).toContain('Unit association not supplied')
  expect(absent).not.toContain('class="setup-unit"')
  expect(absent).toContain('No open placement window supplied')
})
