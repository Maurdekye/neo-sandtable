import { expect, it } from 'vitest'
import { formationTree, filterTree, type FormationNode } from './hierarchy'
import type { UnitView } from './protocol'
const unit = (id: string, parent: string | null): UnitView => ({
  id,
  parent,
  name: id,
  side: 'axis',
  kind: 'infantry',
  size: 'battalion',
  nationality: 'synthetic',
  hex: null,
  detail: null,
})
const members = (nodes: FormationNode[]): string[] =>
  nodes.flatMap((n) => [...(n.unit ? [n.unit.id] : []), ...members(n.children)])
it('retains all disclosed members when OA parents are absent or cyclic', () => {
  const forest = formationTree([
    unit('a', 'b'),
    unit('b', 'a'),
    unit('c', 'missing'),
    unit('d', 'd'),
    unit('e', 'a'),
  ])
  expect(members(forest).sort()).toEqual(['a', 'b', 'c', 'd', 'e'])
  expect(forest.reduce((n, branch) => n + branch.count, 0)).toBe(5)
  expect(forest.find((n) => n.id === 'missing')?.unit).toBeUndefined()
})
it('keeps ancestors of a matched unit and descendants of a matched formation', () => {
  const forest = formationTree([
    unit('army', null),
    unit('brigade', 'army'),
    unit('company', 'brigade'),
    unit('other', 'army'),
  ])
  expect(members(filterTree(forest, 'company'))).toEqual([
    'army',
    'brigade',
    'company',
  ])
  expect(members(filterTree(forest, 'brigade'))).toEqual([
    'army',
    'brigade',
    'company',
  ])
  expect(filterTree(forest, 'hidden-unit')).toEqual([])
})
