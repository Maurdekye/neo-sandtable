import { useSyncExternalStore } from 'react'
import type { Perspective, ServerMessage, Subscribe } from '../protocol'
import { advance, initialState, receive, seek, type ViewerState } from './model'
let state = initialState()
const listeners = new Set<() => void>()
let send: ((request: Subscribe) => void) | undefined
function publish(next: ViewerState) {
  state = next
  listeners.forEach((l) => l())
}
export function deliver(message: ServerMessage) {
  const result = receive(state, message)
  publish(result.state)
  if (result.subscribe) send?.(result.subscribe)
}
export function setTransport(transport: (request: Subscribe) => void) {
  send = transport
  send({ type: 'subscribe', perspective: state.perspective, from_seq: null })
  return () => {
    send = undefined
  }
}
export function useViewer() {
  return useSyncExternalStore(
    (callback) => {
      listeners.add(callback)
      return () => listeners.delete(callback)
    },
    () => state,
  )
}
export const getViewer = () => state
export const actions = {
  clear() {
    publish(initialState(state.perspective))
  },
  connecting() {
    publish({ ...state, connection: 'connecting' })
  },
  perspective(value: Perspective) {
    publish(initialState(value))
    send?.({ type: 'subscribe', perspective: value, from_seq: null })
  },
  pause() {
    publish({ ...state, cursor: state.cursor ?? state.lastSeq, playing: false })
  },
  live() {
    publish({ ...state, cursor: null, playing: false })
  },
  seek(seq: number) {
    publish(seek(state, seq))
  },
  step() {
    publish(
      advance(
        state.cursor === null
          ? { ...state, cursor: state.frames[0]?.seq ?? null }
          : state,
      ),
    )
  },
  play() {
    if (state.cursor === null)
      publish({ ...state, cursor: state.frames[0]?.seq ?? null, playing: true })
    else publish({ ...state, playing: !state.playing })
  },
  speed(value: number) {
    publish({ ...state, speed: value })
  },
  tick() {
    if (state.playing) publish(advance(state))
  },
}
