import type { ServerMessage, Subscribe } from '../protocol'
import { decodeMessage } from './wire'
import { validToken } from '../access'
export type Socket = Pick<
  WebSocket,
  | 'readyState'
  | 'send'
  | 'close'
  | 'onopen'
  | 'onmessage'
  | 'onclose'
  | 'onerror'
>
export interface SocketStatus {
  phase: 'connecting' | 'connected' | 'retrying' | 'stopped'
  message: string
}
export function streamUrl(
  server: string,
  campaign: string,
  token: string,
): string {
  const url = new URL(
    `/api/campaigns/${encodeURIComponent(campaign)}/stream`,
    server,
  )
  if (url.protocol !== 'http:' && url.protocol !== 'https:')
    throw new Error('Server URL must use HTTP or HTTPS')
  if (!validToken(token)) throw new Error('Campaign access required')
  url.searchParams.set('cap', token)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url.href
}
export function createSocketStream(options: {
  url: string
  deliver: (message: ServerMessage) => void
  lastGoodSeq: () => number | null
  status: (status: SocketStatus) => void
  factory?: (url: string) => Socket
}) {
  const factory = options.factory ?? ((url) => new WebSocket(url))
  let socket: Socket | null = null,
    request: Subscribe | null = null,
    timer: ReturnType<typeof setTimeout> | null = null,
    deadline: ReturnType<typeof setTimeout> | null = null,
    stopped = false,
    delay = 500
  function detach() {
    if (deadline !== null) {
      clearTimeout(deadline)
      deadline = null
    }
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
    const old = socket
    socket = null
    if (old) {
      old.onopen = old.onmessage = old.onclose = old.onerror = null
      old.close()
    }
  }
  function retry(message: string) {
    if (stopped) return
    detach()
    options.status({ phase: 'retrying', message })
    const wait = delay
    delay = Math.min(10000, delay * 2)
    timer = setTimeout(() => {
      timer = null
      if (request) request = { ...request, from_seq: options.lastGoodSeq() }
      connect()
    }, wait)
  }
  function connect() {
    if (stopped || !request) return
    options.status({
      phase: 'connecting',
      message: 'Connecting to campaign stream',
    })
    let current: Socket
    try {
      current = factory(options.url)
    } catch {
      retry('Cannot open the campaign stream')
      return
    }
    socket = current
    deadline = setTimeout(
      () => retry('Timed out waiting for the campaign snapshot'),
      30000,
    )
    let acknowledged = false
    current.onopen = () => {
      if (socket === current && !stopped && request) {
        try {
          current.send(JSON.stringify(request))
        } catch {
          retry('Cannot send the campaign subscription')
        }
      }
    }
    current.onmessage = (event) => {
      if (socket !== current || stopped) return
      let message: ServerMessage | null
      try {
        message = decodeMessage(event.data)
      } catch {
        retry('Invalid campaign stream response')
        return
      }
      if (!message) return
      if (
        message.type === 'hello' &&
        message.perspective !== request?.perspective
      ) {
        retry('Server acknowledged a different perspective')
        return
      }
      if (message.type === 'hello') acknowledged = true
      if (message.type === 'snapshot' && !acknowledged) {
        retry('Snapshot arrived before the campaign acknowledgement')
        return
      }
      options.deliver(message)
      // Delivery can synchronously replace this socket after a sequence gap.
      if (socket !== current || stopped) return
      if (
        message.type === 'snapshot' ||
        (message.type === 'hello' && request?.from_seq !== null)
      ) {
        if (deadline !== null) {
          clearTimeout(deadline)
          deadline = null
        }
        delay = 500
        options.status({ phase: 'connected', message: 'Live server stream' })
      }
    }
    current.onclose = (event) => {
      if (socket !== current) return
      if (event.code === 1008) {
        detach()
        options.status({
          phase: 'stopped',
          message: 'This access does not allow the requested perspective',
        })
        return
      }
      retry('Connection closed; reconnecting from the last good event')
    }
    current.onerror = () => {
      if (socket === current) retry('Connection failed; retrying')
    }
  }
  return {
    subscribe(next: Subscribe) {
      if (stopped) return
      // New socket isolates old projections and queued private messages.
      detach()
      request = { ...next }
      delay = 500
      connect()
    },
    close() {
      if (stopped) return
      stopped = true
      detach()
      options.status({ phase: 'stopped', message: 'Stream closed' })
    },
  }
}
