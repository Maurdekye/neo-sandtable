import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  apiRequest,
  captureCredential,
  credentialKey,
  decodeSession,
  mayView,
  serverOrigin,
} from './access'
const token = 'a'.repeat(64)
const origin = 'http://127.0.0.1:3000'
function storage() {
  const data = new Map<string, string>()
  return {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => {
      data.set(k, v)
    },
    removeItem: (k: string) => {
      data.delete(k)
    },
  }
}
afterEach(() => vi.unstubAllGlobals())
describe('capability boundaries', () => {
  it('removes only the capability fragment and restores it only for its issuing origin', () => {
    const saved = storage(),
      replace = vi.fn()
    expect(
      captureCredential(
        origin,
        new URL(`http://localhost:5173/?server=x#cap=${token}&tab=board`),
        saved,
        replace,
      ),
    ).toBe(token)
    expect(replace.mock.calls[0][0]).toBe(
      'http://localhost:5173/?server=x#tab=board',
    )
    expect(
      captureCredential(
        origin,
        new URL('http://localhost:5173/'),
        saved,
        replace,
      ),
    ).toBe(token)
    expect(
      captureCredential(
        'http://127.0.0.1:3001',
        new URL('http://localhost:5173/'),
        saved,
        replace,
      ),
    ).toBe('')
  })
  it('fails closed on malformed or ambiguous fragments instead of reusing an old operator token', () => {
    const saved = storage()
    saved.setItem(credentialKey(origin), token)
    expect(
      captureCredential(
        origin,
        new URL(`http://localhost/#cap=${token}&cap=bad`),
        saved,
        () => {},
      ),
    ).toBe('')
    expect(saved.getItem(credentialKey(origin))).toBeNull()
  })
  it('supports memory-only access when browser storage is disabled', () => {
    const fail = () => {
      throw new Error('disabled')
    }
    expect(
      captureCredential(
        origin,
        new URL(`http://localhost/#cap=${token}`),
        { getItem: fail, setItem: fail, removeItem: fail },
        () => {},
      ),
    ).toBe(token)
  })
  it('rejects nonlocal origins, embedded user info and non-HTTP schemes', () => {
    for (const server of [
      'https://attacker.test',
      'http://localhost.attacker.test',
      'http://user@localhost',
      'file:///tmp',
    ])
      expect(() => serverOrigin(server)).toThrow()
    expect(serverOrigin('http://[::1]:3000/path')).toBe('http://[::1]:3000')
  })
  it('checks server scope and permits only same-side seats for a side capability', () => {
    const session = decodeSession({
      operator: false,
      campaign_id: 'fixture',
      perspective: 'side:axis',
    })
    expect(mayView(session, 'seat:axis.commander')).toBe(true)
    expect(mayView(session, 'seat:commonwealth.commander')).toBe(false)
    expect(mayView(session, 'operator')).toBe(false)
    expect(
      mayView({ ...session, perspective: 'seat:axis.commander' }, 'side:axis'),
    ).toBe(false)
    for (const value of [
      { operator: true, campaign_id: 'id', perspective: 'operator' },
      { operator: false, campaign_id: null, perspective: 'side:axis' },
      { operator: true, campaign_id: null, perspective: 'seat:axis.commander' },
    ])
      expect(() => decodeSession(value)).toThrow()
  })
  it('adds Bearer to every request and rejects redirects or cross-origin endpoints', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue({ ok: true, json: async () => ({ ok: true }) })
    vi.stubGlobal('fetch', fetch)
    await apiRequest({ server: origin, token }, '/api/session')
    expect(
      new Headers(fetch.mock.calls[0][1].headers).get('Authorization'),
    ).toBe(`Bearer ${token}`)
    expect(fetch.mock.calls[0][1]).toMatchObject({
      redirect: 'error',
      credentials: 'omit',
      referrerPolicy: 'no-referrer',
      cache: 'no-store',
    })
    await expect(
      apiRequest(
        { server: origin, token },
        'http://localhost:3001/api/session',
      ),
    ).rejects.toThrow('endpoint')
    await expect(
      apiRequest({ server: origin, token: '' }, '/api/session'),
    ).rejects.toThrow('access')
    expect(fetch).toHaveBeenCalledTimes(1)
  })
  it('reports denials and network failures without leaking a token or server response body', async () => {
    const fetch = vi.fn().mockRejectedValue(new Error(`url?cap=${token}`))
    vi.stubGlobal('fetch', fetch)
    await expect(
      apiRequest({ server: origin, token }, '/api/session'),
    ).rejects.toThrow('Cannot reach')
    fetch.mockResolvedValue({ status: 401, ok: false })
    await expect(
      apiRequest({ server: origin, token }, '/api/session'),
    ).rejects.toThrow('expired or invalid')
    fetch.mockResolvedValue({ status: 403, ok: false })
    await expect(
      apiRequest({ server: origin, token }, '/api/session'),
    ).rejects.toThrow('does not allow')
  })
})

it('invalidates viewer access on a 401 but not on a scoped 403', async () => {
  const denied = vi.fn()
  const fetch = vi.fn().mockResolvedValue({ status: 401, ok: false })
  vi.stubGlobal('fetch', fetch)
  await expect(
    apiRequest(
      { server: origin, token, onUnauthorized: denied },
      '/api/session',
    ),
  ).rejects.toThrow('expired')
  expect(denied).toHaveBeenCalledTimes(1)
  fetch.mockResolvedValue({ status: 403, ok: false })
  await expect(
    apiRequest(
      { server: origin, token, onUnauthorized: denied },
      '/api/session',
    ),
  ).rejects.toThrow('allow')
  expect(denied).toHaveBeenCalledTimes(1)
})

it('accepts a nonempty opaque seat identifier from the authoritative session', () => {
  expect(
    decodeSession({
      operator: false,
      campaign_id: 'fixture',
      perspective: 'seat:custom-seat_1',
    }).perspective,
  ).toBe('seat:custom-seat_1')
})
