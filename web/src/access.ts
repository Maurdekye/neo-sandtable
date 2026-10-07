import { validToken } from './credentialFormat'
import type { Perspective } from './protocol'
export interface Session {
  perspective: Perspective
  campaign_id: string | null
  operator: boolean
}
export interface Access {
  server: string
  token: string
  session: Session
  onUnauthorized?: () => void
}
// A capability is valid only for the backend origin that issued it.
export function serverOrigin(server: string): string {
  const url = new URL(server)
  if (
    !['http:', 'https:'].includes(url.protocol) ||
    url.username ||
    url.password ||
    !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
  )
    throw new Error('Use a local HTTP or HTTPS campaign server')
  return url.origin
}
export const credentialKey = (server: string) =>
  `cna:cap:${serverOrigin(server)}`
export { validToken } from './credentialFormat'
export function captureCredential(
  server: string,
  url: URL,
  storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
  replace: (url: string) => void,
): string {
  const key = credentialKey(server)
  const fragment = new URLSearchParams(url.hash.slice(1))
  if (fragment.has('cap')) {
    const supplied = fragment.getAll('cap')
    fragment.delete('cap')
    url.hash = fragment.toString()
    replace(url.href)
    const token =
      supplied.length === 1 && validToken(supplied[0]) ? supplied[0] : ''
    try {
      storage.removeItem(key)
      if (token) storage.setItem(key, token)
    } catch {
      /* Memory-only access remains usable. */
    }
    return token
  }
  try {
    const token = storage.getItem(key) ?? ''
    return validToken(token) ? token : ''
  } catch {
    return ''
  }
}
export function forgetCredential(server: string) {
  try {
    sessionStorage.removeItem(credentialKey(server))
  } catch {
    /* Storage may be disabled. */
  }
  location.reload()
}
export function mayView(session: Session, perspective: Perspective): boolean {
  if (session.operator) return true
  if (session.perspective === perspective) return true
  if (!session.perspective.startsWith('side:')) return false
  const side = session.perspective.slice(5)
  return perspective.startsWith(`seat:${side}.`)
}
export function decodeSession(value: unknown): Session {
  if (typeof value !== 'object' || value === null)
    throw new Error('Invalid access scope')
  const data = value as Record<string, unknown>
  const p = data.perspective
  if (
    typeof p !== 'string' ||
    !/^(operator|side:(axis|commonwealth)|seat:\S+)$/.test(p) ||
    typeof data.operator !== 'boolean' ||
    !(
      data.campaign_id === null ||
      (typeof data.campaign_id === 'string' && data.campaign_id.length > 0)
    ) ||
    (data.operator
      ? p !== 'operator' || data.campaign_id !== null
      : p === 'operator' || data.campaign_id === null)
  )
    throw new Error('Invalid access scope')
  return {
    perspective: p as Perspective,
    campaign_id: data.campaign_id as string | null,
    operator: data.operator,
  }
}
export async function apiRequest(
  access: Pick<Access, 'server' | 'token' | 'onUnauthorized'>,
  path: string,
  options?: RequestInit,
): Promise<unknown> {
  if (!validToken(access.token)) throw new Error('Campaign access required')
  const origin = serverOrigin(access.server)
  const url = new URL(path, origin)
  if (url.origin !== origin || !url.pathname.startsWith('/api/'))
    throw new Error('Invalid campaign endpoint')
  const headers = new Headers(options?.headers)
  headers.set('Authorization', `Bearer ${access.token}`)
  let response: Response
  try {
    response = await fetch(url, {
      ...options,
      headers,
      redirect: 'error',
      credentials: 'omit',
      referrerPolicy: 'no-referrer',
      cache: 'no-store',
      signal: options?.signal
        ? AbortSignal.any([options.signal, AbortSignal.timeout(10000)])
        : AbortSignal.timeout(10000),
    })
  } catch {
    throw new Error('Cannot reach the campaign server')
  }
  if (response.status === 401) {
    access.onUnauthorized?.()
    throw new Error(
      'Campaign access expired or invalid; open a fresh access link',
    )
  }
  if (response.status === 403)
    throw new Error('This access does not allow the requested operation')
  if (!response.ok)
    throw new Error(`Campaign request failed (${response.status})`)
  try {
    return await response.json()
  } catch {
    throw new Error('Invalid campaign response')
  }
}
