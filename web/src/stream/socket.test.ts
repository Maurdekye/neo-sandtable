import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  createSocketStream,
  streamUrl,
  type Socket,
  type SocketStatus,
} from './socket'
import { decodeMessage } from './wire'
import { initialState, receive } from './model'
import type { ServerMessage, Subscribe } from '../protocol'
const clock = {
  game_turn: 1,
  date: '1940-09-15',
  stage: 'opstage',
  op_stage: 1,
  phase: 'movement_and_combat',
  segment: null,
  step: null,
  phasing: 'axis' as const,
}
const hello: ServerMessage = {
  type: 'hello',
  protocol: 1,
  perspective: 'operator',
  campaign: {
    id: 'fixture',
    scenario_id: 'fixture',
    rules_profile: 'fixture',
    title: 'Fixture',
    seats: [],
  },
}
const snapshot: ServerMessage = {
  type: 'snapshot',
  seq: 7,
  view: { clock, stacks: [], units: {}, markers: [], pending: [] },
}
const subscribe: Subscribe = {
  type: 'subscribe',
  perspective: 'operator',
  from_seq: null,
}
class FakeSocket {
  readyState = 0
  onopen: Socket['onopen'] = null
  onmessage: Socket['onmessage'] = null
  onclose: Socket['onclose'] = null
  onerror: Socket['onerror'] = null
  sent: Subscribe[] = []
  send(data: string) {
    this.sent.push(JSON.parse(data))
  }
  close = vi.fn()
  open() {
    this.readyState = 1
    this.onopen?.call(this as unknown as WebSocket, {} as Event)
  }
  message(message: unknown) {
    this.onmessage?.call(
      this as unknown as WebSocket,
      { data: JSON.stringify(message) } as MessageEvent,
    )
  }
  closed() {
    this.onclose?.call(this as unknown as WebSocket, {} as CloseEvent)
  }
}
function setup() {
  const sockets: FakeSocket[] = [],
    delivered: ServerMessage[] = [],
    statuses: SocketStatus[] = []
  const stream = createSocketStream({
    url: 'ws://localhost/stream',
    deliver: (m) => delivered.push(m),
    lastGoodSeq: () => 9,
    status: (s) => statuses.push(s),
    factory: () => {
      const s = new FakeSocket()
      sockets.push(s)
      return s
    },
  })
  stream.subscribe(subscribe)
  return { stream, sockets, delivered, statuses }
}
beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())
describe('live transport', () => {
  it('builds secure URLs and escapes campaign path components', () => {
    expect(streamUrl('https://example.test/', 'a/b', 'a'.repeat(64))).toBe(
      `wss://example.test/api/campaigns/a%2Fb/stream?cap=${'a'.repeat(64)}`,
    )
    expect(() => streamUrl('file:///tmp', 'id', 'a'.repeat(64))).toThrow('HTTP')
  })
  it('subscribes after opening and accepts a hello then snapshot', () => {
    const h = setup(),
      s = h.sockets[0]
    expect(s.sent).toEqual([])
    s.open()
    expect(s.sent).toEqual([subscribe])
    s.message(hello)
    s.message(snapshot)
    expect(h.delivered).toEqual([hello, snapshot])
    expect(h.statuses.at(-1)?.phase).toBe('connected')
    h.stream.close()
  })
  it('reconnects with exponential backoff and the last good event', () => {
    const h = setup()
    h.sockets[0].closed()
    vi.advanceTimersByTime(499)
    expect(h.sockets).toHaveLength(1)
    vi.advanceTimersByTime(1)
    h.sockets[1].open()
    expect(h.sockets[1].sent[0].from_seq).toBe(9)
    h.sockets[1].closed()
    vi.advanceTimersByTime(999)
    expect(h.sockets).toHaveLength(2)
    vi.advanceTimersByTime(1)
    h.sockets[2].message(hello)
    h.sockets[2].message(snapshot)
    h.sockets[2].closed()
    vi.advanceTimersByTime(500)
    expect(h.sockets).toHaveLength(4)
    h.stream.close()
  })
  it('isolates queued old-perspective messages and cancels retry timers on replacement', () => {
    const h = setup(),
      old = h.sockets[0],
      queued = old.onmessage!
    old.closed()
    h.stream.subscribe({ ...subscribe, perspective: 'side:axis' })
    queued.call(
      old as unknown as WebSocket,
      { data: JSON.stringify(snapshot) } as MessageEvent,
    )
    vi.advanceTimersByTime(10000)
    expect(h.delivered).toEqual([])
    expect(h.sockets).toHaveLength(2)
    h.sockets[1].open()
    expect(h.sockets[1].sent[0].perspective).toBe('side:axis')
    h.stream.close()
  })
  it('backs off invalid payloads, protocol versions and wrong perspectives', () => {
    for (const message of [
      { type: 'snapshot', seq: 0, view: {} },
      { ...hello, protocol: 2 },
      { ...hello, perspective: 'side:axis' },
    ]) {
      const h = setup()
      h.sockets[0].message(message)
      expect(h.delivered).toEqual([])
      expect(h.statuses.at(-1)?.phase).toBe('retrying')
      h.stream.close()
    }
  })
  it('requires hello before snapshot and ignores future envelope types', () => {
    const h = setup()
    h.sockets[0].message({ type: 'future' })
    expect(h.statuses.at(-1)?.phase).toBe('connecting')
    h.sockets[0].message(snapshot)
    expect(h.delivered).toEqual([])
    expect(h.statuses.at(-1)?.phase).toBe('retrying')
    h.stream.close()
  })
  it('replaces the socket synchronously on a sequence gap and delivers recovered state', () => {
    const sockets: FakeSocket[] = []
    let state = initialState()
    const stream = createSocketStream({
      url: 'ws://localhost/',
      lastGoodSeq: () => state.lastSeq,
      status: () => {},
      factory: () => {
        const s = new FakeSocket()
        sockets.push(s)
        return s
      },
      deliver: (m) => {
        const result = receive(state, m)
        state = result.state
        if (result.subscribe) stream.subscribe(result.subscribe)
      },
    })
    stream.subscribe(subscribe)
    sockets[0].open()
    sockets[0].message(hello)
    sockets[0].message(snapshot)
    sockets[0].message({
      type: 'event',
      seq: 9,
      clock,
      event: { kind: 'note', text: 'Gap' },
    })
    expect(state.frames).toEqual([])
    expect(sockets).toHaveLength(2)
    sockets[1].open()
    expect(sockets[1].sent[0].from_seq).toBeNull()
    sockets[1].message(hello)
    sockets[1].message({ ...snapshot, seq: 9 })
    expect(state.lastSeq).toBe(9)
    expect(state.connection).toBe('live')
    stream.close()
  })
  it('retries a socket that never supplies its snapshot', () => {
    const h = setup()
    h.sockets[0].open()
    h.sockets[0].message(hello)
    vi.advanceTimersByTime(30000)
    expect(h.statuses.at(-1)?.message).toContain('Timed out')
    vi.advanceTimersByTime(500)
    expect(h.sockets).toHaveLength(2)
    h.stream.close()
  })
  it('accepts event replay after a resume hello without another snapshot', () => {
    const sockets: FakeSocket[] = []
    let state = initialState()
    const stream = createSocketStream({
      url: 'ws://localhost/',
      lastGoodSeq: () => (state.frames.length ? state.lastSeq : null),
      status: (s) => {
        if (s.phase !== 'connected')
          state = { ...state, connection: 'connecting' }
      },
      factory: () => {
        const s = new FakeSocket()
        sockets.push(s)
        return s
      },
      deliver: (m) => {
        state = receive(state, m).state
      },
    })
    stream.subscribe(subscribe)
    sockets[0].message(hello)
    sockets[0].message(snapshot)
    sockets[0].closed()
    vi.advanceTimersByTime(500)
    sockets[1].open()
    sockets[1].message(hello)
    sockets[1].message({
      type: 'event',
      seq: 8,
      clock,
      event: { kind: 'note', text: 'Replayed' },
    })
    expect(state.lastSeq).toBe(8)
    expect(state.frames).toHaveLength(2)
    vi.advanceTimersByTime(30000)
    expect(sockets).toHaveLength(2)
    stream.close()
  })
  it('cancels all work after close, including queued callbacks', () => {
    const h = setup(),
      queued = h.sockets[0].onmessage!
    h.sockets[0].closed()
    h.stream.close()
    vi.advanceTimersByTime(20000)
    h.stream.subscribe(subscribe)
    queued.call(
      h.sockets[0] as unknown as WebSocket,
      { data: JSON.stringify(hello) } as MessageEvent,
    )
    expect(h.sockets).toHaveLength(1)
    expect(h.delivered).toEqual([])
  })
})
it('rejects unsafe sequence values and malformed transcript content at the JSON boundary', () => {
  for (const seq of ['7', -1, 1.5, Number.MAX_SAFE_INTEGER + 1])
    expect(() => decodeMessage(JSON.stringify({ ...snapshot, seq }))).toThrow(
      'snapshot',
    )
  expect(() =>
    decodeMessage(
      JSON.stringify({
        type: 'transcript',
        seat: 'axis.commander',
        tseq: 1,
        game_seq: 7,
        at: '2026-10-06T00:00:00Z',
        entry: { kind: 'assistant_text', text: {} },
      }),
    ),
  ).toThrow('transcript')
  expect(() => decodeMessage(new Blob())).toThrow('text')
  expect(
    decodeMessage(
      JSON.stringify({
        type: 'event',
        seq: 8,
        clock,
        event: { kind: 'future' },
      }),
    ),
  ).toMatchObject({ seq: 8 })
})

it('halts policy denials without retrying or displaying a server reason', () => {
  const h = setup()
  h.sockets[0].onclose?.call(
    h.sockets[0] as unknown as WebSocket,
    { code: 1008, reason: 'secret capability' } as CloseEvent,
  )
  vi.advanceTimersByTime(60000)
  expect(h.sockets).toHaveLength(1)
  expect(h.statuses.at(-1)?.phase).toBe('stopped')
  expect(JSON.stringify(h.statuses)).not.toContain('secret')
  h.stream.close()
})
it('does not echo token-bearing connection errors', () => {
  const statuses: SocketStatus[] = []
  const stream = createSocketStream({
    url: 'ws://localhost/?cap=secret',
    deliver: () => {},
    lastGoodSeq: () => null,
    status: (s) => statuses.push(s),
    factory: () => {
      throw new Error('ws://localhost/?cap=secret')
    },
  })
  stream.subscribe(subscribe)
  expect(JSON.stringify(statuses)).not.toContain('secret')
  stream.close()
})
