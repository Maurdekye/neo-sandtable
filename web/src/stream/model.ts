import {
  emptyMonitor,
  startMonitor,
  recordMonitor,
  type MonitorState,
} from '../monitoring'
import { recordCommentary, type AcceptedCommentary } from '../commentary'
import { recordStage, summaryFrame, type StageSummary } from '../stageTimeline'
import type {
  CampaignMeta,
  Clock,
  GameEvent,
  Perspective,
  ServerMessage,
  Subscribe,
  TranscriptMessage,
  ViewState,
} from '../protocol'
export interface Frame {
  seq: number
  view: ViewState
  event: GameEvent | null
  moved?: string[]
  hex?: string
  unit_id?: string
}
export function segmentKey(clock: Clock) {
  return [
    clock.game_turn,
    clock.op_stage,
    clock.stage,
    clock.phase,
    clock.segment,
    clock.phasing,
  ].join('|')
}
/** Viewer metadata stays cumulative even when the event that established it leaves the history buffer. */
function movementState(
  view: ViewState,
  last?: Frame,
  event?: GameEvent,
): string[] {
  const moved = new Set(
    last?.moved ??
      Object.values(last?.view.units ?? view.units)
        .filter((u) => u.detail?.moved_this_segment === true)
        .map((u) => u.id),
  )
  if (last && segmentKey(last.view.clock) !== segmentKey(view.clock))
    moved.clear()
  if (event?.kind === 'unit_moved') moved.add(event.unit_id)
  else if (event?.kind === 'unit_updated') {
    if (event.unit.detail?.moved_this_segment === true) moved.add(event.unit.id)
    else if (event.unit.detail?.moved_this_segment === false)
      moved.delete(event.unit.id)
  } else if (event?.kind === 'unit_removed') moved.delete(event.unit_id)
  return [...moved].filter((id) => Boolean(view.units[id]))
}
export interface ViewerState {
  monitoring: MonitorState
  campaign: CampaignMeta | null
  perspective: Perspective
  frames: Frame[]
  stages: StageSummary[]
  archiveFrame: Frame | null
  commentaries: AcceptedCommentary[]
  transcripts: TranscriptMessage[]
  cursor: number | null
  playing: boolean
  speed: number
  lastSeq: number | null
  connection: 'connecting' | 'live' | 'resyncing'
  transcriptSeq: Record<string, number>
}
export const MAX_FRAMES = 600,
  MAX_TRANSCRIPTS = 1200
export function initialState(
  perspective: Perspective = 'operator',
): ViewerState {
  return {
    monitoring: emptyMonitor(),
    campaign: null,
    perspective,
    frames: [],
    stages: [],
    archiveFrame: null,
    commentaries: [],
    transcripts: [],
    cursor: null,
    playing: false,
    speed: 1,
    lastSeq: null,
    connection: 'connecting',
    transcriptSeq: {},
  }
}
/** Apply projected events only. Unknown runtime kinds are deliberately ignored. */
function replaceUnit(
  view: ViewState,
  unit: ViewState['units'][string],
): ViewState {
  const stacks = view.stacks
    .map((s) =>
      s.unit_ids.includes(unit.id)
        ? {
            ...s,
            unit_ids: s.unit_ids.filter((id) => id !== unit.id),
            visible_count:
              s.visible_count === null
                ? null
                : Math.max(0, s.visible_count - 1),
          }
        : s,
    )
    .filter(
      (s) =>
        s.unit_ids.length ||
        s.visible_count === null ||
        (s.visible_count ?? 0) > 0,
    )
  if (unit.hex) {
    const target = stacks.find(
      (s) => s.hex === unit.hex && s.side === unit.side,
    )
    if (target) {
      const index = stacks.indexOf(target)
      stacks[index] = {
        ...target,
        unit_ids: [...target.unit_ids, unit.id],
        visible_count:
          target.visible_count === null ? null : target.visible_count + 1,
      }
    } else
      stacks.push({
        hex: unit.hex,
        side: unit.side,
        unit_ids: [unit.id],
        visible_count: 1,
      })
  }
  return { ...view, units: { ...view.units, [unit.id]: unit }, stacks }
}
export function applyEvent(view: ViewState, event: GameEvent): ViewState {
  switch (event.kind) {
    case 'stack_updated':
      return {
        ...view,
        stacks: [
          ...view.stacks.filter(
            (s) => s.hex !== event.stack.hex || s.side !== event.stack.side,
          ),
          event.stack,
        ],
      }
    case 'stack_removed':
      return {
        ...view,
        stacks: view.stacks.filter(
          (s) => s.hex !== event.hex || s.side !== event.side,
        ),
      }
    case 'unit_moved': {
      const old = view.units[event.unit_id],
        hex = event.path.at(-1)
      return old && hex ? replaceUnit(view, { ...old, hex }) : view
    }
    case 'unit_updated':
      return replaceUnit(view, event.unit)
    case 'unit_removed': {
      const old = view.units[event.unit_id]
      if (!old) return view
      const next = replaceUnit(view, { ...old, hex: null })
      const units = { ...next.units }
      delete units[event.unit_id]
      return { ...next, units }
    }
    case 'phase_changed':
      return { ...view, clock: event.clock }
    case 'decision_opened':
      return {
        ...view,
        pending: [
          ...view.pending.filter((d) => d.id !== event.decision.id),
          event.decision,
        ],
      }
    case 'decision_resolved':
      return {
        ...view,
        pending: view.pending.filter((d) => d.id !== event.decision_id),
      }
    case 'marker_placed':
      return {
        ...view,
        markers: [
          ...view.markers.filter((m) => m.id !== event.marker.id),
          event.marker,
        ],
      }
    case 'marker_removed':
      return {
        ...view,
        markers: view.markers.filter((m) => m.id !== event.marker_id),
      }
    default:
      return view
  }
}
export function receive(
  state: ViewerState,
  message: ServerMessage,
  now = Date.now(),
): { state: ViewerState; subscribe?: Subscribe } {
  const resync = () => ({
    state: {
      ...state,
      frames: [],
      monitoring: emptyMonitor(),
      stages: [],
      archiveFrame: null,
      commentaries: [],
      transcripts: [],
      cursor: null,
      playing: false,
      connection: 'resyncing' as const,
      transcriptSeq: {},
    },
    subscribe: {
      type: 'subscribe' as const,
      perspective: state.perspective,
      from_seq: null,
    },
  })
  if (message.type === 'resync') return resync()
  if (message.type === 'hello') {
    if (message.perspective !== state.perspective || message.protocol !== 1)
      return resync()
    const base =
      state.campaign && state.campaign.id !== message.campaign.id
        ? initialState(state.perspective)
        : state
    return {
      state: {
        ...base,
        campaign: message.campaign,
        connection: base.frames.length ? 'live' : base.connection,
      },
    }
  }
  if (message.type === 'snapshot' && !state.campaign) return { state }
  if (message.type === 'snapshot') {
    const snapshot: Frame = {
      seq: message.seq,
      view: message.view,
      event: null,
      moved: movementState(message.view),
    }
    return {
      state: {
        ...state,
        monitoring: state.transcripts
          .filter((m) => m.game_seq <= message.seq)
          .reduce(
            (monitor, m) => recordMonitor(monitor, m, now),
            startMonitor(message.view.pending, now),
          ),
        frames: [snapshot],
        stages: recordStage([], snapshot),
        commentaries: [],
        archiveFrame: null,
        lastSeq: message.seq,
        connection: 'live',
        cursor: null,
        playing: false,
      },
    }
  }
  if (message.type === 'event') {
    if (
      state.connection !== 'live' ||
      state.lastSeq === null ||
      !state.frames.length
    )
      return { state }
    if (message.seq !== state.lastSeq + 1) return resync()
    const last = state.frames.at(-1)!
    const updatedView = {
      ...applyEvent(last.view, message.event),
      clock: message.clock,
    }
    const frame = {
      seq: message.seq,
      view: updatedView,
      moved: movementState(updatedView, last, message.event),
      event: message.event,
      ...(message.hex !== undefined ? { hex: message.hex } : {}),
      ...(message.unit_id !== undefined ? { unit_id: message.unit_id } : {}),
    }
    const frames = [...state.frames, frame].slice(-MAX_FRAMES)
    return {
      state: {
        ...state,
        frames,
        monitoring: recordMonitor(state.monitoring, message, now),
        stages: recordStage(state.stages, frame, last),
        commentaries: recordCommentary(state.commentaries, frame),
        lastSeq: message.seq,
        cursor:
          state.cursor === null || state.archiveFrame
            ? state.cursor
            : state.cursor < frames[0].seq
              ? frames[0].seq
              : state.cursor,
      },
    }
  }
  const last = state.transcriptSeq[message.seat] ?? 0
  if (message.tseq <= last || state.connection !== 'live') return { state }
  // tseq gaps are possible for a resumed snapshot; protocol has no transcript-resume cursor.
  return {
    state: {
      ...state,
      monitoring: recordMonitor(state.monitoring, message, now),
      transcripts: [...state.transcripts, message].slice(-MAX_TRANSCRIPTS),
      transcriptSeq: { ...state.transcriptSeq, [message.seat]: message.tseq },
    },
  }
}
export function selectedFrame(state: ViewerState): Frame | undefined {
  return state.cursor === null
    ? state.frames.at(-1)
    : (state.frames.find((f) => f.seq === state.cursor) ??
        (state.archiveFrame?.seq === state.cursor
          ? state.archiveFrame
          : undefined) ??
        state.frames[0])
}
export function seek(state: ViewerState, seq: number): ViewerState {
  const archived = !state.frames.some((f) => f.seq === seq)
    ? (summaryFrame(state.stages, seq) ??
      state.commentaries.find((c) => c.frame.seq === seq)?.frame)
    : undefined
  if (archived)
    return { ...state, playing: false, cursor: seq, archiveFrame: archived }
  return {
    ...state,
    archiveFrame: null,
    playing: false,
    cursor:
      seq < (state.frames[0]?.seq ?? seq)
        ? state.frames[0].seq
        : seq > (state.lastSeq ?? seq)
          ? state.lastSeq
          : seq,
  }
}
export function advance(state: ViewerState): ViewerState {
  if (state.cursor === null) return state
  if (state.archiveFrame) return { ...state, playing: false }
  const next = state.frames.find((f) => f.seq > state.cursor!)
  return next ? { ...state, cursor: next.seq } : { ...state, playing: false }
}
