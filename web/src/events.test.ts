import { expect, it } from 'vitest'
import { eventHex, eventText, eventMatches } from './events'
import type { Frame } from './stream/model'
const frame = (event: Frame['event'], hex?: string) => ({ event, hex }) as Frame
it('locates only the projected envelope or intrinsic event location, never a removed unit or prose', () => {
  expect(
    eventHex(frame({ kind: 'note', text: 'Secret unit at C4218' })),
  ).toBeNull()
  expect(
    eventHex(frame({ kind: 'unit_removed', unit_id: 'u', reason: 'loss' })),
  ).toBeNull()
  expect(
    eventHex(
      frame({ kind: 'unit_removed', unit_id: 'u', reason: 'loss' }, 'C4218'),
    ),
  ).toBe('C4218')
  expect(
    eventHex(
      frame({
        kind: 'combat_resolved',
        hex: 'C4219',
        summary: 'barrage',
        detail: null,
      }),
    ),
  ).toBe('C4219')
})
it('preserves actual CP and dice reading without inventing missing values', () => {
  expect(
    eventText({
      kind: 'unit_moved',
      unit_id: 'u',
      path: ['C4218', 'C4219'],
      cp_spent: 1.25,
    }),
  ).toContain('1.25 CP spent')
  expect(
    eventText({
      kind: 'unit_moved',
      unit_id: 'u',
      path: ['C4218'],
      cp_spent: null,
    }),
  ).not.toContain('CP spent')
  expect(
    eventText({
      kind: 'dice_rolled',
      dice: [2, 5],
      purpose: 'barrage',
      reading: 25,
      rule: 'land:12.1',
    }),
  ).toContain('reading 25')
  expect(
    eventMatches({ kind: 'note', text: 'private stop' }, 'unit_moved'),
  ).toBe(true)
})

it('describes empty-reason sync removal as visibility, not a loss', () => {
  expect(eventText({ kind: 'unit_removed', unit_id: 'face', reason: '' })).toBe(
    'face counter no longer visible',
  )
  expect(
    eventText({ kind: 'unit_removed', unit_id: 'face', reason: 'combat loss' }),
  ).toContain('combat loss')
})
