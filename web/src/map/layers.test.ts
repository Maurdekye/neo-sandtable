import { expect, it } from 'vitest'
import { boundary, edges, parseLayers, rows, status } from './layers'
import { center, HEX_SIZE, type Hex } from './fixture'
const a: Hex = { id: 'a', q: 0, r: 0, terrain: 'unclassified' },
  b: Hex = { id: 'b', q: 1, r: 0, terrain: 'unclassified' },
  c: Hex = { id: 'c', q: 0, r: 1, terrain: 'unclassified' }
const membership = new Map([a, b, c].map((h) => [h.id, h]))
const lineHeader = 'from_hex,to_hex,kind,src,review_batch\n'
const sideHeader =
  'hex_id,direction,neighbour_id,feature,high_side,src,review_batch\n'
const coverHeader = 'layer,hex_id,neighbour_id,src,review_batch\n'
it('distinguishes positive, explicitly surveyed absence and unknown for each kind', () => {
  const layers = parseLayers(
    lineHeader + 'a,b,road,land:8.13,fixture\n',
    sideHeader,
    coverHeader +
      'line:road,a,b,land:8.13,fixture\nline:track,a,b,land:8.13,fixture\n',
    membership,
  )
  expect(status(layers, 'line:road', 'a|b')).toBe('present')
  expect(status(layers, 'line:track', 'a|b')).toBe('surveyed: none')
  expect(status(layers, 'line:pipeline', 'a|b')).toBe('unknown')
  expect(status(layers, 'line:road', 'a|c')).toBe('unknown')
})
it('keeps reviewed cell coverage separate from edges and rejects unsupported geometry/coverage', () => {
  const layers = parseLayers(
    lineHeader,
    sideHeader,
    coverHeader + 'terrain,a,,land:4.1,fixture\n',
    membership,
  )
  expect(layers.coverage.get('terrain')?.has('a')).toBe(true)
  expect(status(layers, 'line:road', 'a|b')).toBe('unknown')
  expect(() =>
    parseLayers(
      lineHeader + 'a,b,road,land:8.13,x',
      sideHeader,
      coverHeader,
      membership,
    ),
  ).toThrow('outside explicit coverage')
  expect(() =>
    parseLayers(
      lineHeader,
      sideHeader,
      coverHeader + 'line:road,a,,land:8.13,x',
      membership,
    ),
  ).toThrow('cell/edge mismatch')
  expect(() =>
    parseLayers(
      lineHeader,
      sideHeader + 'a,E,b,slope,,land:8.13,x',
      coverHeader + 'side:slope,a,b,land:8.13,x',
      membership,
    ),
  ).toThrow('high side')
})
it('constructs unique physical edges and exact shared boundary endpoints', () => {
  const result = edges([a, b, c])
  expect(result).toHaveLength(3)
  const edge = result.find((e) => e.key === 'a|b')!,
    geometry = boundary(edge),
    ca = center(a),
    cb = center(b)
  expect(geometry.mid.x).toBeCloseTo((ca.x + cb.x) / 2)
  expect(geometry.start.x).toBeCloseTo(geometry.end.x)
  expect(
    Math.hypot(
      geometry.end.x - geometry.start.x,
      geometry.end.y - geometry.start.y,
    ),
  ).toBeCloseTo(HEX_SIZE)
  expect(geometry.normal.x).toBe(1)
})
it('reads quoted CSV records without breaking review notes', () => {
  expect(rows('id,note\r\na,"survey, ""review"""\r\n')).toEqual([
    { id: 'a', note: 'survey, "review"' },
  ])
})
