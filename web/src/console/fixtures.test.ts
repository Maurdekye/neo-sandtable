import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import fixture from './fixtures/graziani-spaces.json'
import { ActionField } from './ActionField'
import { decodeSchema, defaultDraft, type Schema } from './schema'

const nodes = (s: Schema): Schema[] => [
  s,
  ...(s.anyOf?.flatMap(nodes) ?? []),
  ...Object.values(s.properties ?? {}).flatMap(nodes),
  ...(s.items ? nodes(s.items) : []),
]
describe('actual scripted Graziani action-space renderer fixtures', () => {
  it('records a completed no-paid source run with rule citations', () => {
    expect(fixture.completed).toBe(true)
    expect(fixture.paid_calls).toBe(0)
    expect(fixture.controller).toBe('legal_random')
    expect(fixture.source_commit).toMatch(/^[0-9a-f]{40}$/)
    expect(fixture.samples.some((s) => s.kind === 'cna.setup.unit')).toBe(true)
    expect(fixture.samples.some((s) => s.kind === 'cna.movement.orders')).toBe(
      true,
    )
    expect(fixture.samples.every((s) => s.rules.length > 0)).toBe(true)
  })
  for (const [index, sample] of fixture.samples.entries()) {
    it(`renders every nested schema in ${sample.kind} variant ${index + 1}`, () => {
      const schema = decodeSchema(sample.space)
      for (const node of nodes(schema)) {
        const html = renderToStaticMarkup(
          createElement(ActionField, {
            schema: node,
            value: defaultDraft(node),
            label: 'Fixture field',
            onChange: () => {},
            onPick: () => {},
            onInspect: () => {},
          }),
        )
        expect(html).toContain('Fixture field')
        expect(html).not.toContain('<script')
      }
    })
  }
})
