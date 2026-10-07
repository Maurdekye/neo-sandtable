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

it('rejects malformed optional envelope locators but accepts absent ones', () => {
  const m = {
    type: 'event',
    seq: 1,
    clock: { game_turn: 1, date: '', stage: 'setup', phase: 'setup' },
    event: { kind: 'note', text: 'Fixture' },
  }
  expect(decodeMessage(JSON.stringify(m))).toBeTruthy()
  expect(
    decodeMessage(JSON.stringify({ ...m, hex: 'C4218', unit_id: 'u' })),
  ).toBeTruthy()
  expect(() => decodeMessage(JSON.stringify({ ...m, hex: 42 }))).toThrow()
  expect(() =>
    decodeMessage(JSON.stringify({ ...m, unit_id: { id: 'private' } })),
  ).toThrow()
})

it('rejects missing monitored seat status and missing resolution seat', () => {
  const seat = {
    id: 'axis.commander',
    side: 'axis',
    role: 'commander',
    status: 'deciding',
    controller: { kind: 'llm-cli', label: 'Fixture model' },
  }
  const packet = (s: unknown) =>
    JSON.stringify({
      type: 'hello',
      protocol: 1,
      perspective: 'operator',
      campaign: {
        id: 'test',
        title: 'Test',
        rules_profile: 'test',
        scenario_id: 'test',
        seats: [s],
      },
    })
  expect(decodeMessage(packet(seat))).toBeTruthy()
  expect(() => decodeMessage(packet({ ...seat, status: undefined }))).toThrow()
  expect(() =>
    decodeMessage(
      packet({ ...seat, controller: { ...seat.controller, kind: 'unknown' } }),
    ),
  ).toThrow()
  expect(() =>
    decodeMessage(
      JSON.stringify({
        type: 'event',
        seq: 1,
        clock: { game_turn: 1, date: '', stage: 'setup', phase: 'setup' },
        event: {
          kind: 'decision_resolved',
          decision_id: 'test',
          summary: 'Accepted',
        },
      }),
    ),
  ).toThrow()
})

it('validates flat nullable measured usage and rejects negative, missing or imprecise fields', () => {
  const entry = {
    kind: 'usage_snapshot',
    controller_epoch: 1,
    revision: 1,
    provider: null,
    model: null,
    attempts: 1,
    completed: 0,
    input_tokens: null,
    output_tokens: 0,
    cache_read_tokens: null,
    cache_creation_tokens: null,
    reasoning_tokens: null,
    reported_cost_usd: null,
    incomplete_turns: 1,
  }
  const packet = (e: unknown) =>
    JSON.stringify({
      type: 'transcript',
      seat: 'axis.commander',
      tseq: 1,
      game_seq: 1,
      at: '2026-10-07T00:00:00Z',
      entry: e,
    })
  expect(decodeMessage(packet(entry))).toBeTruthy()
  for (const bad of [
    { input_tokens: -1 },
    { input_tokens: 1.5 },
    { output_tokens: undefined },
    { reported_cost_usd: -0.01 },
    { reported_cost_usd: '0.003' },
    { revision: Number.MAX_SAFE_INTEGER + 1 },
    { model: 42 },
  ])
    expect(() => decodeMessage(packet({ ...entry, ...bad }))).toThrow()
})
