import { expect, it } from 'vitest'
import data from './data/rules.json'
import { clockAnchor } from './ruleAnchor'
import type { Clock } from './protocol'
it('exports registry paraphrases and authoritative per-step counts', () => {
  expect(data.rules['land:7.11'].title).toContain('Initiative')
  expect(data.rules['land:7.11'].summary.length).toBeGreaterThan(20)
  const report = data.coverage.graziani
  expect(report.implemented + report.unsupported + report.missing).toBe(
    report.applicable,
  )
  expect(
    report.by_anchor.some((r) => r.anchor === 'opstage.initiative_declaration'),
  ).toBe(true)
  for (const row of report.by_anchor)
    expect(row.implemented + row.unsupported + row.missing).toBe(row.applicable)
})
it('reconstructs timing anchors without duplicating a pre-turn stage', () => {
  const clock: Clock = {
    game_turn: 1,
    date: '1940-09-15',
    stage: 'opstage',
    phase: 'movement_and_combat',
    segment: 'movement',
    step: null,
    op_stage: 1,
    phasing: 'axis',
  }
  expect(
    clockAnchor({
      ...clock,
      stage: 'opstage',
      phase: 'movement_and_combat',
      segment: 'movement',
      step: null,
    }),
  ).toBe('opstage.movement_and_combat.movement')
  expect(
    clockAnchor({
      ...clock,
      stage: 'naval_convoy',
      phase: 'naval_convoy',
      segment: null,
      step: null,
    }),
  ).toBe('naval_convoy')
})
