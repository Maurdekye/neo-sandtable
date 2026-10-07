import { decodeSchema, type Schema, type Value } from './schema'
export interface SeatRequest {
  id: string
  seat: string
  kind: string
  summary: string
  revision: number
  rules: string[]
  schema: Schema
}
export interface SeatState {
  epoch: number
  paused: boolean
  failure: string | null
  controller: string | null
  pending: SeatRequest[]
}
export class ConsoleError extends Error {
  readonly status: number
  constructor(message: string, status: number) {
    super(message)
    this.status = status
  }
}
const record = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v)
export function readRequest(
  v: unknown,
  seat: string,
  exported?: unknown,
): SeatRequest {
  if (
    !record(v) ||
    v.seat !== seat ||
    typeof v.id !== 'string' ||
    typeof v.kind !== 'string' ||
    typeof v.summary !== 'string' ||
    typeof v.revision !== 'number' ||
    !Number.isSafeInteger(v.revision) ||
    v.revision < 0 ||
    !Array.isArray(v.rules) ||
    !v.rules.every((x) => typeof x === 'string')
  )
    throw new Error('Invalid seat decision')
  return {
    id: v.id,
    seat,
    kind: v.kind,
    summary: v.summary,
    revision: v.revision,
    rules: v.rules,
    schema: decodeSchema(exported ?? v.space),
  }
}
export function readSeatState(v: unknown, pending: SeatRequest[]): SeatState {
  if (
    !record(v) ||
    typeof v.controller_epoch !== 'number' ||
    !Number.isSafeInteger(v.controller_epoch) ||
    v.controller_epoch < 0 ||
    typeof v.paused !== 'boolean' ||
    !(v.failure === null || typeof v.failure === 'string') ||
    !Array.isArray(v.pending)
  )
    throw new Error('Invalid seat observation')
  const binding = record(v.controller)
    ? v.controller
    : record(v.binding) && record(v.binding.controller)
      ? v.binding.controller
      : null
  return {
    epoch: v.controller_epoch,
    paused: v.paused,
    failure: v.failure,
    controller: typeof binding?.kind === 'string' ? binding.kind : null,
    pending,
  }
}
export function captureSeatLink(url: URL, replace: (url: string) => void) {
  const fragment = new URLSearchParams(url.hash.slice(1)),
    tokens = fragment.getAll('cap')
  fragment.delete('cap')
  url.hash = fragment.toString()
  replace(url.href)
  const token =
    tokens.length === 1 && /^[a-f0-9]{64}$/.test(tokens[0]) ? tokens[0] : ''
  const serverUrl = new URL(url.origin)
  if (
    url.searchParams.has('server') &&
    new URL(url.searchParams.get('server')!).origin !== url.origin
  )
    throw new Error('The seat console must use its own server origin')
  if (
    !['http:', 'https:'].includes(serverUrl.protocol) ||
    !['localhost', '127.0.0.1', '[::1]'].includes(serverUrl.hostname) ||
    serverUrl.username ||
    serverUrl.password
  )
    throw new Error('Use a local campaign server')
  if (!token)
    throw new Error(
      'Open the seat console link printed by the launcher. Reloading requires a fresh link; the capability is held only in memory.',
    )
  return {
    token,
    server: serverUrl.origin,
    campaign: url.searchParams.get('campaign'),
    seat: url.searchParams.get('seat'),
  }
}
export function createSeatClient(
  link: ReturnType<typeof captureSeatLink>,
  fetcher: typeof fetch = fetch,
) {
  let campaign = '',
    seat = '',
    disposed = false
  const requests = new Set<AbortController>()
  const spaces = new Map<string, SeatRequest>()
  const call = async (path: string, body?: unknown): Promise<unknown> => {
    if (disposed) throw new Error('Console access closed')
    const abort = new AbortController()
    requests.add(abort)
    try {
      const response = await fetcher(new URL(path, link.server), {
        method: body === undefined ? 'GET' : 'POST',
        headers: {
          Authorization: `Bearer ${link.token}`,
          ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.any([abort.signal, AbortSignal.timeout(15000)]),
        credentials: 'omit',
        redirect: 'error',
        referrerPolicy: 'no-referrer',
        cache: 'no-store',
      })
      const value: unknown = await response.json()
      if (!response.ok)
        throw new ConsoleError(
          record(value) && typeof value.error === 'string'
            ? value.error
            : `Seat request failed (${response.status})`,
          response.status,
        )
      return value
    } finally {
      requests.delete(abort)
    }
  }
  const root = () => {
    if (!campaign || !seat) throw new Error('Seat scope not established')
    return `/api/campaigns/${encodeURIComponent(campaign)}/seats/${encodeURIComponent(seat)}`
  }
  return {
    async session() {
      const v = await call('/api/session')
      if (
        !record(v) ||
        v.operator !== false ||
        typeof v.perspective !== 'string' ||
        !v.perspective.startsWith('seat:') ||
        typeof v.campaign_id !== 'string' ||
        !v.campaign_id
      )
        throw new Error('This console requires a seat capability')
      const ownSeat = v.perspective.slice(5)
      if (
        !/^(axis|commonwealth)\.[a-z_]+$/.test(ownSeat) ||
        (link.campaign && link.campaign !== v.campaign_id) ||
        (link.seat && link.seat !== ownSeat)
      )
        throw new Error('The link does not match this seat capability')
      campaign = v.campaign_id
      seat = ownSeat
      return { campaign, seat, perspective: `seat:${seat}` as const }
    },
    async observe() {
      const value = await call(`${root()}/observe`)
      if (!record(value) || !Array.isArray(value.pending))
        throw new Error('Invalid pending decisions')
      const pending: SeatRequest[] = []
      for (const entry of value.pending) {
        if (
          !record(entry) ||
          entry.seat !== seat ||
          typeof entry.id !== 'string'
        )
          throw new Error('Invalid pending seat scope')
        const stamp = `${entry.id}:${entry.revision}`
        let hydrated = spaces.get(stamp)
        if (!hydrated) {
          const action = await call(
            `${root()}/decisions/${encodeURIComponent(entry.id)}/actions`,
          )
          if (!record(action)) throw new Error('Invalid action space response')
          hydrated = readRequest(action.request, seat, action.action_schema)
          spaces.set(`${hydrated.id}:${hydrated.revision}`, hydrated)
          if (spaces.size > 128) spaces.delete(spaces.keys().next().value!)
        }
        pending.push(hydrated)
      }
      return readSeatState(value, pending)
    },
    async actions(id: string) {
      const v = await call(
        `${root()}/decisions/${encodeURIComponent(id)}/actions`,
      )
      if (!record(v)) throw new Error('Invalid action space response')
      return readRequest(v.request, seat, v.action_schema)
    },
    inspect(target: string) {
      return call(`${root()}/inspect/${encodeURIComponent(target)}`)
    },
    validate(id: string, action: Value, epoch: number, revision: number) {
      return call(`${root()}/decisions/${encodeURIComponent(id)}/validate`, {
        action,
        controller_epoch: epoch,
        decision_revision: revision,
      })
    },
    submit(
      request: SeatRequest,
      action: Value,
      epoch: number,
      key: string,
      explanation: string,
    ) {
      return call(
        `${root()}/decisions/${encodeURIComponent(request.id)}/submit`,
        {
          decision_id: request.id,
          seat,
          controller_epoch: epoch,
          decision_revision: request.revision,
          idempotency_key: key,
          action,
          public_explanation: explanation.trim() || null,
        },
      )
    },
    close() {
      disposed = true
      requests.forEach((x) => x.abort())
      requests.clear()
    },
  }
}
export type SeatClient = ReturnType<typeof createSeatClient>
