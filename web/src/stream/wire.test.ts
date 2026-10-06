import { expect, it } from 'vitest'
import { decodeMessage } from './wire'
it('rejects malformed OA parents and detail fields before hierarchy rendering', () => {
  const unit = {
    id: 'u',
    side: 'axis',
    name: 'Fixture',
    kind: 'infantry',
    size: 'battalion',
    nationality: 'synthetic',
    hex: null,
    parent: null,
    detail: null,
  }
  const packet = (value: unknown) =>
    JSON.stringify({
      type: 'event',
      seq: 1,
      clock: { game_turn: 1, date: '', stage: 'setup', phase: 'setup' },
      event: { kind: 'unit_updated', unit: value },
    })
  expect(decodeMessage(packet(unit))).toBeTruthy()
  expect(() =>
    decodeMessage(packet({ ...unit, parent: { secret: 'invalid' } })),
  ).toThrow()
  expect(() =>
    decodeMessage(packet({ ...unit, detail: ['invalid'] })),
  ).toThrow()
})
