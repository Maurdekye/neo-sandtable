import { useEffect, useState } from 'react'
import type { GameEvent, Perspective } from './protocol'
import { Board } from './map/Board'
import { counterSvg } from './map/counters'
import { HEX_BY_ID, INITIAL_HEX, TERRAIN } from './map/fixture'
import { actions, deliver, setTransport, useViewer } from './stream/store'
import { selectedFrame } from './stream/model'
import { Transcripts } from './Transcripts'
function eventText(event: GameEvent): string {
  switch (event.kind) {
    case 'stack_updated':
      return `${event.stack.side} stack at ${event.stack.hex}`
    case 'stack_removed':
      return `${event.side} stack left ${event.hex}`
    case 'unit_moved':
      return `${event.unit_id} → ${event.path.at(-1)}`
    case 'phase_changed':
      return `Phase · ${event.clock.segment ?? event.clock.phase}`
    case 'combat_resolved':
    case 'decision_resolved':
      return event.summary
    case 'decision_opened':
      return event.decision.summary
    case 'note':
      return event.text
    case 'unit_updated':
      return `${event.unit.name} updated`
    case 'unit_removed':
      return `${event.unit_id} removed · ${event.reason}`
    case 'dice_rolled':
      return `${event.purpose} · ${event.dice.join(', ')}`
    case 'marker_placed':
      return `${event.marker.kind} placed`
    case 'marker_removed':
      return `${event.marker_id} removed`
    default:
      return 'Unknown event'
  }
}
function eventHex(event: GameEvent) {
  return event.kind === 'stack_updated'
    ? event.stack.hex
    : event.kind === 'stack_removed'
      ? event.hex
      : event.kind === 'unit_moved'
        ? event.path.at(-1)
        : event.kind === 'combat_resolved'
          ? event.hex
          : event.kind === 'marker_placed'
            ? event.marker.hex
            : event.kind === 'unit_updated'
              ? event.unit.hex
              : null
}
export function App() {
  const state = useViewer(),
    frame = selectedFrame(state),
    view = frame?.view
  const [selected, setSelected] = useState<string | null>(INITIAL_HEX),
    [unitId, setUnitId] = useState<string | null>(null),
    [focus, setFocus] = useState<{ hex: string; nonce: number } | null>(null)
  const [eventFilter, setEventFilter] = useState('all'),
    [campaignPaused, setCampaignPaused] = useState(false),
    [control, setControl] = useState<((paused: boolean) => void) | null>(null)
  useEffect(() => {
    let disposed = false,
      stop = () => {},
      disconnect = () => {}
    if (import.meta.env.DEV)
      void import('./mock/generator').then(({ createMockStream }) => {
        if (disposed) return
        const transport = createMockStream(deliver)
        disconnect = setTransport(transport.subscribe)
        stop = transport.close
        setControl(() => transport.setPaused)
      })
    return () => {
      disposed = true
      disconnect()
      stop()
    }
  }, [])
  useEffect(() => {
    const timer = window.setInterval(actions.tick, 900 / state.speed)
    return () => window.clearInterval(timer)
  }, [state.speed])
  const hex = selected ? HEX_BY_ID.get(selected) : undefined,
    stacks = view?.stacks.filter((s) => s.hex === selected) ?? []
  const unit =
    unitId && view?.units[unitId]?.hex === selected
      ? view.units[unitId]
      : undefined
  const transcriptMessages = state.transcripts.filter(
    (m) => state.cursor === null || m.game_seq <= (frame?.seq ?? 0),
  )
  const events = state.frames
    .filter(
      (f) =>
        f.event &&
        (state.cursor === null || f.seq <= (frame?.seq ?? 0)) &&
        (eventFilter === 'all' || f.event.kind === eventFilter),
    )
    .slice(-60)
    .reverse()
  const index = Math.max(
    0,
    state.frames.findIndex((f) => f.seq === frame?.seq),
  )
  function choose(id: string) {
    setSelected(id)
    setUnitId(null)
  }
  function locate(id: string) {
    choose(id)
    setFocus({ hex: id, nonce: Date.now() })
  }
  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <div className="brand-mark">⬡</div>
          <div>
            <strong>neo-sandtable</strong>
            <span>CAMPAIGN WATCH</span>
          </div>
        </div>
        <div className="campaign-title">
          <strong>{state.campaign?.title ?? 'Campaign viewer'}</strong>
          <small>
            {import.meta.env.DEV
              ? 'Development fixture · no CNA adjudication'
              : 'Server transport not connected yet'}
          </small>
        </div>
        <div className="clock">
          <span>{view?.clock.date ?? '—'}</span>
          <strong>
            TURN {view?.clock.game_turn ?? '—'} · OP{' '}
            {view?.clock.op_stage ?? '—'}
          </strong>
          <small>
            {view
              ? `${view.clock.phase.replaceAll('_', ' ')} · ${view.clock.segment ?? view.clock.stage}`
              : 'Connecting'}
          </small>
        </div>
        <span
          className={`status ${state.cursor !== null ? 'history' : ''}`}
          data-testid="playback-status"
        >
          {state.cursor !== null
            ? `HISTORY #${String(frame?.seq ?? 0)}`
            : state.connection === 'live'
              ? '● LIVE'
              : state.connection.toUpperCase()}
        </span>
      </header>
      <div className="viewbar">
        <span className="eyebrow">PERSPECTIVE</span>
        <select
          aria-label="Perspective"
          value={state.perspective}
          onChange={(e) => {
            actions.perspective(e.target.value as Perspective)
            setUnitId(null)
            setCampaignPaused(false)
            control?.(false)
          }}
        >
          <option value="operator">Operator · OMNISCIENT</option>
          <option value="side:axis">Axis side</option>
          <option value="side:commonwealth">Commonwealth side</option>
          <option value="seat:axis.commander">Seat · Axis commander</option>
          <option value="seat:commonwealth.front_line">
            Seat · Commonwealth front line
          </option>
          <option value="seat:axis.logistics">Seat · Axis logistics</option>
        </select>
        <span className="view-note">
          {state.perspective === 'operator'
            ? 'OMNISCIENT · all authorized data'
            : 'Projection supplied by mock server'}
        </span>
        <span className="grow" />
        <button
          disabled={
            state.perspective !== 'operator' ||
            !control ||
            state.cursor !== null
          }
          onClick={() => {
            setCampaignPaused(!campaignPaused)
            control?.(!campaignPaused)
          }}
        >
          {campaignPaused ? 'Resume campaign (mock)' : 'Pause campaign (mock)'}
        </button>
      </div>
      <main className="workspace">
        <aside className="formations">
          <div className="panel-heading">
            <span className="eyebrow">FORMATIONS</span>
          </div>
          {['axis', 'commonwealth'].map((side) => (
            <div className="formation" key={side}>
              <h3>
                <i className={`dot ${side}`} />
                {side}
              </h3>
              {Object.values(view?.units ?? {})
                .filter((u) => u.side === side)
                .map((u) => (
                  <button
                    key={u.id}
                    onClick={() => {
                      if (u.hex) {
                        locate(u.hex)
                        setUnitId(u.id)
                      }
                    }}
                  >
                    {u.name}
                  </button>
                ))}
            </div>
          ))}
          <div className="overlay-controls">
            <span className="eyebrow">OVERLAYS</span>
            {['Supply', 'Transport', 'Air missions', 'Control'].map((label) => (
              <label key={label}>
                <input type="checkbox" disabled />
                {label}
                <small>pending data</small>
              </label>
            ))}
          </div>
        </aside>
        <Board
          view={view}
          selected={selected}
          focus={focus}
          onSelect={choose}
        />
        <Transcripts
          seats={state.campaign?.seats ?? []}
          messages={transcriptMessages}
        />
        <aside className="inspector">
          <div className="panel-heading">
            <span className="eyebrow">INSPECTOR</span>
          </div>
          <h2>{selected ?? 'Select a hex'}</h2>
          {hex && (
            <>
              <p className="muted">
                {TERRAIN[hex.terrain].label} · axial {hex.q}, {hex.r}
              </p>
              <div className="inspect-coordinates">
                {hex.label ?? 'Real coordinate grid / synthetic units'}
              </div>
            </>
          )}
          {stacks.map((s) => (
            <section key={s.side}>
              <h3>
                {s.side} · {s.visible_count ?? 'unknown'} units
              </h3>
              {!s.unit_ids.length && (
                <p className="muted">
                  Presence disclosed; composition unavailable.
                </p>
              )}
              {s.unit_ids.map((id) => {
                const u = view?.units[id]
                return (
                  u && (
                    <button
                      className={`unit-row ${unitId === id ? 'selected' : ''}`}
                      key={id}
                      onClick={() => setUnitId(id)}
                    >
                      <img
                        alt={`${u.kind} ${u.size} counter`}
                        src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(counterSvg(u))}`}
                      />
                      <span>
                        {u.name}
                        <small>{u.size}</small>
                      </span>
                    </button>
                  )
                )
              })}
            </section>
          ))}
          {!stacks.length && <p className="empty">No visible stack.</p>}
          {unit && (
            <section className="unit-detail">
              <h3>{unit.name}</h3>
              <dl>
                <dt>Kind</dt>
                <dd>{unit.kind}</dd>
                <dt>Nationality</dt>
                <dd>{unit.nationality}</dd>
                <dt>Formation</dt>
                <dd>{unit.parent ?? '—'}</dd>
              </dl>
              <pre>{JSON.stringify(unit.detail, null, 2)}</pre>
            </section>
          )}
        </aside>
      </main>
      <footer className="timeline">
        <div className="playback">
          <span className="eyebrow">PLAYBACK</span>
          <button onClick={actions.pause} disabled={!frame}>
            Pause playback
          </button>
          <button onClick={actions.step} disabled={!frame}>
            Step →
          </button>
          <button onClick={actions.play} disabled={!frame}>
            {state.playing ? 'Stop replay' : 'Play history'}
          </button>
          <select
            aria-label="Replay speed"
            value={state.speed}
            onChange={(e) => actions.speed(Number(e.target.value))}
          >
            {[0.5, 1, 2, 4].map((v) => (
              <option key={v} value={v}>
                {v}×
              </option>
            ))}
          </select>
          <input
            type="range"
            aria-label="History"
            min="0"
            max={Math.max(0, state.frames.length - 1)}
            value={index}
            onChange={(e) => {
              const f = state.frames[Number(e.target.value)]
              if (f) actions.seek(f.seq)
            }}
          />
          <span className="sequence">
            #{String(frame?.seq ?? 0)} / {String(state.lastSeq ?? 0)}
          </span>
          <button className="live-button" onClick={actions.live}>
            Return to live
          </button>
        </div>
        <div className="feed-header">
          <span className="eyebrow">EVENT FEED</span>
          <select
            aria-label="Event filter"
            value={eventFilter}
            onChange={(e) => setEventFilter(e.target.value)}
          >
            <option value="all">All events</option>
            <option value="unit_moved">Movement</option>
            <option value="combat_resolved">Combat</option>
            <option value="phase_changed">Phase</option>
          </select>
          <small>
            {state.frames.length} / 600 frames retained · transcripts bounded at
            1,200
          </small>
        </div>
        <div className="event-feed">
          {events.map((f) => {
            const h = eventHex(f.event!)
            return (
              <button
                key={String(f.seq)}
                disabled={!h}
                onClick={() => {
                  if (h) locate(h)
                }}
              >
                <span>#{String(f.seq)}</span>
                <strong>{f.event!.kind.replaceAll('_', ' ')}</strong>
                <small>{eventText(f.event!)}</small>
                {h && <em>Locate ↗</em>}
              </button>
            )
          })}
          {!events.length && (
            <p className="empty">Waiting for campaign events.</p>
          )}
        </div>
      </footer>
    </div>
  )
}
