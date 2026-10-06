import { expect, it } from 'vitest'
import {
  center,
  HEXES,
  HEX_BY_ID,
  hexAt,
  parseMap,
  syntheticMap,
} from './fixture'
it('consumes canonical membership and real seam aliases without adding masked hexes', () => {
  expect(HEXES).toHaveLength(7023)
  expect(HEX_BY_ID.get('C4218')).toMatchObject({
    q: 74,
    r: 21,
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

it('preserves unclassified cells and cited flags without inferring terrain', () => {
  const csv =
    'hex_id,section,printed_first,printed_second,q,r,terrain,flags,src\nC4218,C,42,18,74,21,unclassified,,land:4.1\nC4219,C,42,19,75,21,clear,reviewed,land:4.1;land:8.37'
  expect(parseMap(csv)).toEqual([
    {
      id: 'C4218',
      q: 74,
      r: 21,
      terrain: 'unclassified',
      flags: '',
      src: 'land:4.1',
    },
    {
      id: 'C4219',
      q: 75,
      r: 21,
      terrain: 'clear',
      flags: 'reviewed',
      src: 'land:4.1;land:8.37',
    },
  ])
})
