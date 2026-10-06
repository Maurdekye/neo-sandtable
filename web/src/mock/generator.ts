/** DEV-ONLY synthetic transport. No CNA rule values, adjudication, or real agent sessions. */
import type {
  CampaignMeta,
  Clock,
  GameEvent,
  Perspective,
  SeatInfo,
  ServerMessage,
  Subscribe,
  TranscriptEntry,
  UnitView,
  ViewState,
} from '../protocol'
import { denseView } from './dense'
import { HEX_BY_ID, demoHex } from '../map/fixture'
import { applyEvent } from '../stream/model'
const seats: SeatInfo[] = [
  {
    id: 'axis.commander',
    side: 'axis',
    role: 'commander',
    controller: { kind: 'llm-cli', label: 'Mock CLI · A' },
    status: 'deciding',
  },
  {
    id: 'commonwealth.front_line',
    side: 'commonwealth',
    role: 'front_line',
    controller: { kind: 'llm-cli', label: 'Mock CLI · B' },
    status: 'deciding',
  },
  {
    id: 'axis.logistics',
    side: 'axis',
    role: 'logistics',
    controller: { kind: 'system1', label: 'Mock System 1' },
    status: 'idle',
  },
]
const clock: Clock = {
  game_turn: 1,
  date: '1940-09-15',
  stage: 'opstage',
  op_stage: 1,
  phase: 'movement_and_combat',
  segment: 'movement',
  step: null,
  phasing: 'axis',
}
const kinds = [
  'infantry',
  'armor',
  'artillery',
  'recce',
  'hq',
  'engineers',
  'anti_tank',
  'aa',
  'motorized',
  'mechanized',
  'truck',
]
function fixtureView(): ViewState {
  const units: Record<string, UnitView> = {}
  for (let i = 0; i < 18; i++) {
    const side = i < 10 ? 'axis' : 'commonwealth',
      hex =
        i < 5
          ? demoHex(16, 12)
          : i < 10
            ? demoHex(19, 15)
            : i < 14
              ? demoHex(25, 12)
              : demoHex(28, 16)
    const id = `demo-unit-${i}`
    units[id] = {
      id,
      side,
      name: `Demo ${i + 1} · ${kinds[i % kinds.length]}`,
      kind: kinds[i % kinds.length],
      size: i % 3 ? 'battalion' : 'brigade',
      nationality: side === 'axis' ? 'italian' : 'british',
      hex,
      parent: `${side}-demo-formation`,
      detail: {
        strength: `${2 + (i % 6)}–${4 + (i % 4)}`,
        provenance: 'Synthetic display values, not CNA data',
      },
    }
  }
  const stacks = [
    demoHex(16, 12),
    demoHex(19, 15),
    demoHex(25, 12),
    demoHex(28, 16),
  ].map((hex) => {
    const list = Object.values(units).filter((u) => u.hex === hex)
    return {
      hex,
      side: list[0].side,
      unit_ids: list.map((u) => u.id),
      visible_count: list.length,
    }
  })
  return {
    clock,
    stacks,
    units,
    markers: [
      {
        id: 'demo-objective',
        kind: 'objective',
        hex: demoHex(25, 12),
        side: null,
        label: 'Demo objective',
      },
    ],
    pending: [],
  }
}
/** The mock server projects BEFORE delivery. Renderer consumes only this authorized payload. */
export function project(view: ViewState, perspective: Perspective): ViewState {
  if (perspective === 'operator') return view
  const side = perspective.startsWith('side:')
    ? perspective.slice(5)
    : seats.find((s) => `seat:${s.id}` === perspective)?.side
  const units = Object.fromEntries(
    Object.entries(view.units).filter(([, u]) => u.side === side),
  )
  return {
    ...view,
    units,
    markers: view.markers.map((m) =>
      m.side && m.side !== side ? { ...m, label: null } : m,
    ),
    stacks: view.stacks.map((s) =>
      s.side === side ? s : { ...s, unit_ids: [], visible_count: null },
    ),
    pending: view.pending.filter((d) =>
      perspective.startsWith('seat:')
        ? perspective === `seat:${d.seat}`
        : d.seat.startsWith(`${side}.`),
    ),
  }
}
export function createMockStream(deliver: (message: ServerMessage) => void) {
  let seq = 0,
    view =
      new URLSearchParams(location.search).get('fixture') === 'dense'
        ? denseView(clock)
        : fixtureView(),
    perspective: Perspective = 'operator',
    paused = new URLSearchParams(location.search).get('paused') === '1'
  const tseq: Record<string, number> = {}
  const allowedSeats = () =>
    seats.filter(
      (s) =>
        perspective === 'operator' ||
        perspective === `side:${s.side}` ||
        perspective === `seat:${s.id}`,
    )
  const transcript = (seat: string, entry: TranscriptEntry) => {
    if (!allowedSeats().some((s) => s.id === seat)) return
    tseq[seat] = (tseq[seat] ?? 0) + 1
    deliver({
      type: 'transcript',
      seat,
      tseq: tseq[seat],
      at: new Date().toISOString(),
      game_seq: seq,
      entry,
    })
  }
  function subscribe(request: Subscribe) {
    if (perspective !== request.perspective) seq = 0
    perspective = request.perspective as Perspective
    const campaign: CampaignMeta = {
      id: 'synthetic-demo',
      scenario_id: 'synthetic',
      rules_profile: 'synthetic-display-only',
      title: 'Desert watch · synthetic campaign',
      seats: allowedSeats(),
    }
    deliver({ type: 'hello', protocol: 1, campaign, perspective })
    deliver({ type: 'snapshot', seq, view: project(view, perspective) })
    allowedSeats().forEach((s) => {
      transcript(s.id, {
        kind: 'system',
        text: `${s.controller?.label} session started. Synthetic transcript.`,
      })
      if (s.controller?.kind === 'system1') {
        transcript(s.id, {
          kind: 'system1_query',
          question: 'Which synthetic convoy should be prioritized?',
          options: ['Coastal route', 'Inland route'],
        })
        transcript(s.id, {
          kind: 'system1_answer',
          choice: 'Coastal route',
          scores: { coastal: 0.81, inland: 0.19 },
        })
      } else {
        transcript(s.id, {
          kind: 'assistant_text',
          text: 'Inspect the forward stacks before choosing the next move.',
        })
        transcript(s.id, {
          kind: 'reasoning',
          text: 'Synthetic reasoning: inspect the route; the real terrain classification is incomplete.',
        })
        transcript(s.id, {
          kind: 'tool_call',
          call_id: `inspect-${seq}`,
          tool: 'observe',
          args: { area: 'front' },
        })
        transcript(s.id, {
          kind: 'tool_result',
          call_id: `inspect-${seq}`,
          ok: true,
          summary: 'Forward positions returned.',
          detail: { source: 'synthetic fixture' },
        })
      }
    })
  }
  function tick() {
    if (paused) return
    seq++
    const count = seq % 100000
    const visibleUnits = Object.values(project(view, perspective).units).filter(
      (u) => u.hex && HEX_BY_ID.has(u.hex),
    )
    const unit = visibleUnits[count % visibleUnits.length],
      origin = unit.hex!,
      q = 14 + (count % 17),
      r = 12 + (count % 5)
    const nextClock: Clock = {
      ...view.clock,
      op_stage: (Math.floor(count / 12) % 3) + 1,
      segment: count % 6 === 3 ? 'combat' : 'movement',
      phasing: count % 2 ? 'axis' : 'commonwealth',
    }
    const event: GameEvent =
      count % 12 === 5
        ? {
            kind: 'decision_opened',
            decision: {
              id: `demo-decision-${Math.floor(count / 12)}`,
              seat: allowedSeats()[0].id,
              kind: 'movement',
              summary: 'Choose the next synthetic move',
              opened_seq: seq,
            },
          }
        : count % 12 === 8
          ? {
              kind: 'decision_resolved',
              decision_id: `demo-decision-${Math.floor(count / 12)}`,
              seat: allowedSeats()[0].id,
              summary: 'Synthetic movement decision accepted',
            }
          : count % 6 === 0
            ? { kind: 'phase_changed', clock: nextClock }
            : count % 6 === 3
              ? {
                  kind: 'combat_resolved',
                  summary:
                    'Synthetic combat demonstration · no rule adjudication',
                  hex: origin,
                  detail: null,
                }
              : {
                  kind: 'unit_moved',
                  unit_id: unit.id,
                  path: [origin, demoHex(q, r)],
                  cp_spent: null,
                }
    view = { ...applyEvent(view, event), clock: nextClock }
    deliver({ type: 'event', seq, clock: nextClock, event })
    const seat = allowedSeats()[count % allowedSeats().length]
    if (!seat) return
    if (seat.controller?.kind === 'system1') {
      transcript(seat.id, {
        kind: 'system1_query',
        question: 'Prioritize next supply delivery?',
        options: ['Forward', 'Reserve'],
      })
      transcript(seat.id, {
        kind: 'system1_answer',
        choice: 'Forward',
        scores: { forward: 0.74, reserve: 0.26 },
      })
    } else {
      const n = Number((tseq[seat.id] ?? 0) % 4),
        call_id = `move-${seat.id}-${Math.floor(Number(tseq[seat.id] ?? 0) / 4)}`
      transcript(
        seat.id,
        n === 0
          ? {
              kind: 'assistant_text',
              text: `Assessing the next synthetic move at event ${seq}.`,
            }
          : n === 1
            ? {
                kind: 'tool_call',
                call_id,
                tool: 'describe_actions',
                args: { decision_id: `demo-${seq}` },
              }
            : n === 2
              ? {
                  kind: 'tool_result',
                  call_id,
                  ok: true,
                  summary: 'Two legal fixture actions returned.',
                  detail: { source: 'synthetic' },
                }
              : {
                  kind: 'decision_submitted',
                  decision_id: `demo-${seq}`,
                  summary: 'Advance one synthetic hex.',
                },
      )
    }
  }
  const interval = window.setInterval(tick, 900)
  return {
    subscribe,
    setPaused(value: boolean) {
      paused = value
    },
    close() {
      window.clearInterval(interval)
    },
  }
}
