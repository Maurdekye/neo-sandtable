import { describe, expect, it, vi } from 'vitest'
import {
  concrete,
  decodeSchema,
  defaultDraft,
  fieldKind,
  passOnly,
  reconcileDraft,
  validateDraft,
} from './schema'
import { captureSeatLink, createSeatClient, readRequest } from './client'
const record = {
  type: 'object',
  properties: {
    unit: { type: 'string', enum: ['own.a', 'own.b'], 'x-kind': 'unit' },
    optional: {
      anyOf: [{ type: 'string', minLength: 2, maxLength: 4 }, { type: 'null' }],
    },
    count: { type: 'integer', minimum: 0, maximum: 2 },
    flag: { type: 'boolean' },
    path: {
      type: 'array',
      items: { type: 'string', pattern: '^[A-E][0-9]{4}$' },
      maxItems: 2,
      'x-kind': 'path',
      'x-from': 'C4120',
    },
    names: {
      type: 'array',
      items: { type: 'string', minLength: 1, maxLength: 3 },
      minItems: 1,
      maxItems: 3,
    },
  },
  required: ['unit', 'count', 'flag', 'path', 'names'],
  additionalProperties: false,
}
const space = decodeSchema({
  anyOf: [record, { type: 'null', description: 'Pass: done' }],
  'x-context': { unit: 'own.a' },
})
describe('seat action schema', () => {
  it('validates all core shapes, unicode character counts and nested optional omission', () => {
    expect(validateDraft(space, null)).toEqual([])
    const valid = {
      unit: 'own.a',
      count: 2,
      flag: false,
      path: ['C4220'],
      names: ['x'],
      optional: '😀😀',
    }
    expect(validateDraft(space, valid)).toEqual([])
    expect(validateDraft(space, { ...valid, optional: null })).toEqual([])
    const invalid = {
      ...valid,
      unit: 'enemy',
      count: 2.5,
      flag: 1,
      path: ['bad'],
      names: [],
      optional: 'a',
      unexpected: 0,
    }
    expect(validateDraft(space, invalid)).toHaveLength(7)
    expect(fieldKind(concrete(space).properties!.unit)).toBe('unit')
    expect(fieldKind(concrete(space).properties!.path)).toBe('path')
  })
  it('retains valid draft portions when revised without retaining forbidden enum values', () => {
    const next = decodeSchema({
      ...record,
      properties: {
        ...record.properties,
        unit: { type: 'string', enum: ['own.b'] },
      },
    })
    expect(
      reconcileDraft(next, {
        unit: 'own.a',
        count: 1,
        flag: true,
        path: ['C4220'],
        names: ['x'],
        optional: 'ok',
        extra: 3,
      }),
    ).toEqual({
      unit: '',
      count: 1,
      flag: true,
      path: ['C4220'],
      names: ['x'],
      optional: 'ok',
    })
    expect(
      validateDraft(
        next,
        reconcileDraft(next, {
          unit: 'own.a',
          count: 1,
          flag: true,
          path: [],
          names: ['x'],
        }),
      ),
    ).toHaveLength(1)
  })
  it('recognizes only genuinely pass-only spaces and does not confuse empty lists with pass', () => {
    expect(
      passOnly(
        decodeSchema({
          anyOf: [{ type: 'string', enum: [] }, { type: 'null' }],
        }),
      ),
    ).toBe(true)
    expect(passOnly(space)).toBe(false)
    expect(
      passOnly(
        decodeSchema({
          anyOf: [
            {
              type: 'array',
              items: { type: 'string', enum: [] },
              minItems: 0,
              maxItems: 1,
            },
            { type: 'null' },
          ],
        }),
      ),
    ).toBe(false)
    expect(
      validateDraft(concrete(space), defaultDraft(concrete(space))).length,
    ).toBeGreaterThan(0)
  })
  it('fails closed on unknown constraints, unsupported alternatives and excessive nesting', () => {
    for (const s of [
      { type: 'string', format: 'email' },
      { type: 'boolean', maxLength: 4 },
      { anyOf: [{ type: 'boolean' }, { type: 'null' }], enum: ['a'] },
      { anyOf: [{ type: 'string' }, { type: 'integer' }] },
      { type: 'integer', minimum: 3, maximum: 1 },
      { type: 'string', pattern: '(a+)+$' },
    ])
      expect(() => decodeSchema(s)).toThrow()
    let nested: unknown = { type: 'boolean' }
    for (let i = 0; i < 25; i++) nested = { type: 'array', items: nested }
    expect(() => decodeSchema(nested)).toThrow()
  })
})
describe('console credential and route isolation', () => {
  it('captures and removes the fragment without reading or writing storage', () => {
    const replace = vi.fn(),
      url = new URL(
        `http://127.0.0.1:3000/console.html?campaign=c&seat=axis.commander#cap=${'a'.repeat(64)}&keep=1`,
      )
    const link = captureSeatLink(url, replace)
    expect(link.token).toBe('a'.repeat(64))
    expect(replace.mock.calls[0][0]).not.toContain('cap=')
    expect(url.hash).toBe('#keep=1')
    expect(() => captureSeatLink(url, replace)).toThrow('fresh link')
    expect(() =>
      captureSeatLink(
        new URL(`https://evil.example/console.html#cap=${'a'.repeat(64)}`),
        replace,
      ),
    ).toThrow('local')
    expect(() =>
      captureSeatLink(
        new URL(
          `http://localhost/console.html#cap=${'a'.repeat(64)}&cap=${'b'.repeat(64)}`,
        ),
        replace,
      ),
    ).toThrow('fresh link')
  })
  it('rejects operator and mismatched-seat sessions before any campaign request', async () => {
    const fetcher = vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            operator: true,
            perspective: 'operator',
            campaign_id: null,
          }),
        ),
    )
    const c = createSeatClient(
      {
        token: 'a'.repeat(64),
        server: 'http://127.0.0.1:3000',
        campaign: 'c',
        seat: 'axis.commander',
      },
      fetcher,
    )
    await expect(c.session()).rejects.toThrow('seat capability')
    expect(fetcher).toHaveBeenCalledTimes(1)
    await expect(c.observe()).rejects.toThrow('scope not established')
    expect(fetcher).toHaveBeenCalledTimes(1)
  })
  it('hydrates pending schema from authorized actions and submits exact revision/epoch', async () => {
    const calls: { url: string; body: unknown }[] = []
    const request = {
      id: 'axis.commander-1',
      seat: 'axis.commander',
      kind: 'sample',
      summary: 'Choose',
      revision: 3,
      rules: ['land:8.1'],
      space: { schema: { type: 'choice', options: [] }, pass: null },
    }
    const fetcher = vi.fn(
      async (url: URL | RequestInfo, init?: RequestInit) => {
        const path = new URL(String(url)).pathname
        calls.push({
          url: path,
          body: init?.body ? JSON.parse(String(init.body)) : undefined,
        })
        expect(new Headers(init?.headers).get('Authorization')).toBe(
          `Bearer ${'a'.repeat(64)}`,
        )
        return new Response(
          JSON.stringify(
            path === '/api/session'
              ? {
                  operator: false,
                  perspective: 'seat:axis.commander',
                  campaign_id: 'c',
                }
              : path.endsWith('/observe')
                ? {
                    controller_epoch: 2,
                    controller: { kind: 'human' },
                    paused: false,
                    failure: null,
                    pending: [request],
                  }
                : path.endsWith('/actions')
                  ? {
                      request,
                      action_schema: { type: 'string', enum: ['done'] },
                    }
                  : { valid: true },
          ),
        )
      },
    )
    const c = createSeatClient(
      {
        token: 'a'.repeat(64),
        server: 'http://127.0.0.1:3000',
        campaign: 'c',
        seat: 'axis.commander',
      },
      fetcher,
    )
    await c.session()
    const state = await c.observe()
    expect(state.controller).toBe('human')
    expect(state.pending[0].schema.enum).toEqual(['done'])
    await c.validate(request.id, 'done', 2, 3)
    await c.submit(state.pending[0], 'done', 2, 'same-key', 'hello')
    expect(calls.at(-1)!.body).toMatchObject({
      decision_revision: 3,
      controller_epoch: 2,
      idempotency_key: 'same-key',
      public_explanation: 'hello',
    })
    expect(
      calls.every(
        (c) =>
          c.url === '/api/session' ||
          c.url.startsWith('/api/campaigns/c/seats/axis.commander/'),
      ),
    ).toBe(true)
    c.close()
    await expect(c.inspect('unit')).rejects.toThrow('closed')
  })
  it('rejects foreign decision rows and preserves the engine preflight message', async () => {
    expect(() =>
      readRequest(
        { id: 'other', seat: 'commonwealth.commander' },
        'axis.commander',
      ),
    ).toThrow()
    const fetcher = vi.fn(
      async (url: URL | RequestInfo) =>
        new Response(
          JSON.stringify(
            String(url).endsWith('/api/session')
              ? {
                  operator: false,
                  perspective: 'seat:axis.commander',
                  campaign_id: 'c',
                }
              : { error: 'action: fuel unavailable' },
          ),
          { status: String(url).endsWith('/api/session') ? 200 : 409 },
        ),
    )
    const c = createSeatClient(
      {
        token: 'a'.repeat(64),
        server: 'http://localhost',
        campaign: null,
        seat: null,
      },
      fetcher,
    )
    await c.session()
    await expect(c.validate('own', null, 2, 3)).rejects.toThrow(
      'action: fuel unavailable',
    )
  })
})
