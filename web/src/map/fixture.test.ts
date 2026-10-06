import { expect, it } from 'vitest'
import { center, HEXES, HEX_BY_ID, hexAt, syntheticMap } from './fixture'
it('consumes canonical membership and real seam aliases without adding masked hexes', () => {
  expect(HEXES).toHaveLength(7023)
  expect(HEX_BY_ID.get('C4218')).toMatchObject({
    q: 74,
    r: 21,
    terrain: 'unclassified',
  })
  expect(HEX_BY_ID.get('D4200')).toBe(HEX_BY_ID.get('C4233'))
  expect(HEX_BY_ID.has('A9999')).toBe(false)
  const h = HEX_BY_ID.get('C4218')!
  const p = center(h)
  expect(hexAt(p.x, p.y)).toBe(h)
  expect(hexAt(-10000, -10000)).toBeUndefined()
})
it('retains a separate 10k benchmark fixture covering every synthetic terrain', () => {
  const fixture = syntheticMap()
  expect(fixture).toHaveLength(10000)
  expect(new Set(fixture.map((h) => h.terrain)).size).toBe(6)
})
