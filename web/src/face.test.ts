import { createElement } from 'react'
import { expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import type { UnitView } from './protocol'
import { printedFace, isOpponent } from './face'
import { PrintedFace } from './PrintedFace'
import { counterSvg } from './map/counters'
const face: UnitView = {
  id: 'enemy',
  side: 'commonwealth',
  name: 'Printed enemy',
  kind: 'infantry',
  size: 'battalion',
  nationality: 'british',
  hex: 'C4220',
  parent: null,
  detail: { counter: '1/KRRC', stacking_points: 3 },
}
it('renders printed points and designation independently of private strength', () => {
  const svg = counterSvg(face)
  expect(svg).toContain('1/KRRC')
  expect(svg).toContain('SP 3')
  expect(
    counterSvg({
      ...face,
      detail: { ...face.detail, strength: 999, cp_spent: 7, engaged: true },
    }),
  ).toBe(svg)
  expect(
    renderToStaticMarkup(createElement(PrintedFace, { unit: face })),
  ).toContain('3 stacking points')
})
it('escapes counter text as literal SVG and React text', () => {
  const hostile = {
    ...face,
    detail: { counter: '<img src=x>', stacking_points: 0 },
  }
  expect(counterSvg(hostile)).toContain('&lt;img src=x&gt;')
  expect(counterSvg(hostile)).not.toContain('<img')
  const html = renderToStaticMarkup(
    createElement(PrintedFace, { unit: hostile }),
  )
  expect(html).toContain('&lt;img src=x&gt;')
  expect(html).not.toContain('<img')
  expect(html).toContain('0 stacking points')
})
it('keeps absent values absent and a breakdown face free of inferred contents', () => {
  const marker = {
    ...face,
    name: 'Broken-down vehicles',
    kind: 'broken_vehicle',
    size: 'marker',
    detail: null,
  }
  expect(printedFace(marker)).toEqual({ counter: null, stackingPoints: null })
  expect(counterSvg(marker)).toContain('>BD</text>')
  expect(counterSvg(marker)).not.toContain('SP 0')
  const html = renderToStaticMarkup(
    createElement(PrintedFace, { unit: marker }),
  )
  expect(html).not.toContain('stacking points')
  expect(
    printedFace({ ...face, detail: { stacking_points: '3' } }).stackingPoints,
  ).toBeNull()
})
it('treats side and seat views as their own side, and operator as full access', () => {
  expect(isOpponent('commonwealth', 'seat:axis.commander')).toBe(true)
  expect(isOpponent('commonwealth', 'side:axis')).toBe(true)
  expect(isOpponent('axis', 'seat:axis.front_line')).toBe(false)
  expect(isOpponent('commonwealth', 'side:commonwealth')).toBe(false)
  expect(isOpponent('commonwealth', 'operator')).toBe(false)
})
