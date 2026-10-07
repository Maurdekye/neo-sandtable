import { SeatMonitor } from './SeatMonitor'
import { StageTimeline } from './StageOverview'
import { useEffect, useMemo, useState } from 'react'
import type { Perspective } from './protocol'
import { Board } from './map/Board'
import { Formations } from './Formations'
import { locationLabel, unitLocation } from './location'
import { StackList } from './StackList'
import { PendingDecisions } from './PendingDecisions'
import { RulesCoverage } from './Rules'
import { EventFeed } from './EventFeed'
import { eventMatches } from './events'
import { StatusBadges } from './StatusBadges'
import {
  TerrainCoverageControls,
  TerrainCoverageInspector,
} from './map/TerrainCoveragePanel'
import { LayerControls, LayerInspector } from './map/LayerControls'
import { movedUnits } from './movement'
import { isPlacement, placementDestinations, placementHexes } from './setup'
import { DEFAULT_LAYERS } from './map/layers'
import { HEX_BY_ID, INITIAL_HEX, TERRAIN } from './map/fixture'
import {
  actions,
  deliver,
  getViewer,
  setTransport,
  useViewer,
} from './stream/store'
import { selectedFrame } from './stream/model'
import { Transcripts } from './Transcripts'
import { createSocketStream, streamUrl } from './stream/socket'
import { CampaignControl, CampaignChooser } from './Campaigns'
import { AccessGate } from './AccessGate'
import { forgetCredential, mayView, type Access } from './access'
const denseFixture =
  import.meta.env.DEV &&
  new URLSearchParams(location.search).get('fixture') === 'dense'
const requestedCampaign = new URLSearchParams(location.search).get('campaign')
const serverUrl =
  new URLSearchParams(location.search).get('server') ?? location.origin
const mockMode =
  import.meta.env.DEV &&
  !requestedCampaign &&
  !new URLSearchParams(location.search).has('server') &&
  !new URLSearchParams(location.hash.slice(1)).has('cap')
function sameLocation(a: string | null, b: string | null) {
  return (
    a === b ||
    Boolean(
      a &&
      b &&
      HEX_BY_ID.has(a) &&
      HEX_BY_ID.get(a)?.id === HEX_BY_ID.get(b)?.id,
    )
  )
}
export function App() {
  return mockMode ? (
    <Viewer />
  ) : (
    <AccessGate server={serverUrl} campaign={requestedCampaign}>
      {(access) => <Viewer access={access} />}
    </AccessGate>
  )
}
function Viewer({ access }: { access?: Access }) {
  const campaignId = access?.session.campaign_id ?? requestedCampaign
  const allowed = (perspective: Perspective) =>
    !access || mayView(access.session, perspective)
  const state = useViewer(),
    frame = selectedFrame(state),
    view = frame?.view
  const [selected, setSelected] = useState<string | null>(INITIAL_HEX),
    [unitId, setUnitId] = useState<string | null>(null),
    [focus, setFocus] = useState<{
      hex: string
      nonce: number
      bounds?: string[]
    } | null>(null)
  const [layers, setLayers] = useState(DEFAULT_LAYERS)
  const [terrainCoverage, setTerrainCoverage] = useState(false)
  const [moving, setMoving] = useState(
    !matchMedia('(prefers-reduced-motion: reduce)').matches,
  )
  const moved = useMemo(
    () =>
      movedUnits(
        view,
        state.archiveFrame ? [state.archiveFrame] : state.frames,
        frame?.seq ?? null,
      ),
    [view, state.frames, state.archiveFrame, frame?.seq],
  )
  const [placementId, setPlacementId] = useState<string | null>(null)
  const placementDecision =
    view?.pending.find((d) => isPlacement(d) && d.id === placementId) ??
    view?.pending.find(isPlacement)
  const placement = useMemo(
    () =>
      placementDecision && placementDestinations(placementDecision) !== null
        ? {
            label: placementDecision.summary,
            hexes: placementHexes(placementDecision),
          }
        : null,
    [placementDecision],
  )
  const [eventFilter, setEventFilter] = useState('all'),
    [campaignPaused, setCampaignPaused] = useState(
      mockMode && new URLSearchParams(location.search).get('paused') === '1',
    ),
    [control, setControl] = useState<((paused: boolean) => void) | null>(null),
    [transportNote, setTransportNote] = useState(
      mockMode
        ? 'Development fixture · no CNA adjudication'
        : 'Open a campaign with ?campaign=<id>',
    ),
    [liveFocus, setLiveFocus] = useState(false)
  useEffect(() => {
    let disposed = false,
      stop = () => {},
      disconnect = () => {}
    if (access) actions.perspective(access.session.perspective)
    if (campaignId && access) {
      try {
        const server = serverUrl
        const transport = createSocketStream({
          url: streamUrl(server, campaignId, access.token),
          deliver,
          lastGoodSeq: () =>
            getViewer().frames.length ? getViewer().lastSeq : null,
          status: (status) => {
            if (!disposed) {
              setTransportNote(status.message)
              if (status.phase === 'stopped') actions.clear()
              else if (status.phase !== 'connected') actions.connecting()
            }
          },
        })
        disconnect = setTransport(transport.subscribe)
        stop = transport.close
      } catch {
        setTransportNote('Cannot open the campaign stream')
      }
    } else if (mockMode)
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
  }, [access, campaignId])
  useEffect(() => {
    const timer = window.setInterval(actions.tick, 900 / state.speed)
    return () => window.clearInterval(timer)
  }, [state.speed])
  useEffect(() => {
    if ((!mockMode || denseFixture) && view && !liveFocus) {
      const hex =
        view.stacks.find((s) => HEX_BY_ID.has(s.hex))?.hex ??
        view.markers.find((m) => HEX_BY_ID.has(m.hex))?.hex
      if (hex) {
        setSelected(hex)
        setFocus({
          hex,
          nonce: Date.now(),
          bounds: [
            ...view.stacks.map((s) => s.hex),
            ...view.markers.map((m) => m.hex),
          ],
        })
        setLiveFocus(true)
      }
    }
  }, [view, liveFocus])
  const hex = selected ? HEX_BY_ID.get(selected) : undefined,
    stacks = view?.stacks.filter((s) => sameLocation(s.hex, selected)) ?? []
  const unit =
    unitId &&
    view?.units[unitId] &&
    sameLocation(view.units[unitId].hex, selected)
      ? view.units[unitId]
      : undefined
  const transcriptMessages = state.transcripts.filter(
    (m) => state.cursor === null || m.game_seq <= (frame?.seq ?? 0),
  )
  const events = (state.archiveFrame ? [state.archiveFrame] : state.frames)
    .filter(
      (f) =>
        f.event &&
        (state.cursor === null || f.seq <= (frame?.seq ?? 0)) &&
        eventMatches(f.event, eventFilter),
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
  function showPlacement(id: string) {
    setPlacementId(id)
    const request = view?.pending.find((d) => d.id === id && isPlacement(d))
    const hexes = placementHexes(request)
    if (hexes.length)
      setFocus({ hex: hexes[0], nonce: Date.now(), bounds: hexes })
  }
  function chooseUnit(id: string) {
    const unit = view?.units[id]
    if (!unit) return
    setSelected(unit.hex)
    setUnitId(id)
    if (unit.hex && HEX_BY_ID.has(unit.hex))
      setFocus({ hex: unit.hex, nonce: Date.now() })
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
          <small>{transportNote}</small>
        </div>
        <div className="clock">
          <span>{view?.clock.date || '—'}</span>
          <strong>
            TURN {view?.clock.game_turn ?? '—'} · OP{' '}
            {view?.clock.op_stage ?? '—'}
          </strong>
          <small>
            {view
              ? [
                  view.clock.stage,
                  view.clock.phase,
                  view.clock.segment,
                  view.clock.step,
                  view.clock.phasing,
                ]
                  .filter(Boolean)
                  .join(' · ')
                  .replaceAll('_', ' ')
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
            if (!allowed(e.target.value as Perspective)) return
            actions.perspective(e.target.value as Perspective)
            setUnitId(null)
            setCampaignPaused(false)
            control?.(false)
          }}
        >
          <option disabled={!allowed('operator')} value="operator">
            Operator · OMNISCIENT
          </option>
          <option disabled={!allowed('side:axis')} value="side:axis">
            Axis side
          </option>
          <option
            disabled={!allowed('side:commonwealth')}
            value="side:commonwealth"
          >
            Commonwealth side
          </option>
          {state.perspective.startsWith('seat:') &&
            !state.campaign?.seats.some(
              (seat) => `seat:${seat.id}` === state.perspective,
            ) && <option value={state.perspective}>{state.perspective}</option>}
          {state.campaign?.seats.map((seat) => (
            <option
              disabled={!allowed(`seat:${seat.id}`)}
              key={seat.id}
              value={`seat:${seat.id}`}
            >
              Seat · {seat.id.replaceAll('_', ' ')}
            </option>
          ))}
        </select>
        <span className="view-note">
          {state.perspective === 'operator'
            ? 'OMNISCIENT · all authorized data'
            : 'Projection supplied by server'}
        </span>
        <span className="grow" />
        {access && !campaignId && <CampaignChooser access={access} />}
        {access && (
          <button onClick={() => forgetCredential(access.server)}>
            Forget access
          </button>
        )}
        {access && campaignId && (
          <CampaignControl
            access={access}
            campaign={campaignId}
            perspective={state.perspective}
            history={state.cursor !== null}
          />
        )}
        {mockMode && (
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
            {!mockMode
              ? 'Campaign control pending API'
              : campaignPaused
                ? 'Resume campaign (mock)'
                : 'Pause campaign (mock)'}
          </button>
        )}
      </div>
      <SeatMonitor state={state} access={access} />
      <main className="workspace">
        <aside className="formations">
          <div className="panel-heading">
            <span className="eyebrow">FORMATIONS</span>
          </div>
          <PendingDecisions
            pending={view?.pending ?? []}
            units={view?.units ?? {}}
            onUnit={chooseUnit}
            activePlacement={placementDecision?.id}
            onPlacement={showPlacement}
            onHex={locate}
          />
          <LayerControls options={layers} onChange={setLayers} />
          <TerrainCoverageControls
            enabled={terrainCoverage}
            onChange={setTerrainCoverage}
            onHex={locate}
          />
          <label className="motion-control">
            <input
              type="checkbox"
              checked={moving}
              onChange={(event) => setMoving(event.target.checked)}
            />
            Animate disclosed movement
          </label>
          <RulesCoverage clock={view?.clock} />
          <Formations units={view?.units ?? {}} onUnit={chooseUnit} />
          <section className="formation">
            <h3>Objectives & markers</h3>
            {view?.markers.map((marker) => (
              <button key={marker.id} onClick={() => locate(marker.hex)}>
                {marker.label ?? marker.kind}
                <small> · {marker.side ?? 'unheld'}</small>
              </button>
            ))}
            {!view?.markers.length && <small>No disclosed markers</small>}
          </section>
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
          layers={layers}
          frames={state.frames}
          seq={frame?.seq ?? null}
          scope={
            state.perspective +
            ':' +
            (state.campaign?.id ?? '') +
            (state.cursor === null ? ':live' : ':history')
          }
          moving={moving}
          allowBatch={state.cursor === null || state.playing}
          moved={moved}
          placement={placement}
          terrainCoverage={terrainCoverage}
        />
        <Transcripts
          commentaries={state.commentaries}
          seats={state.campaign?.seats ?? []}
          messages={transcriptMessages}
          mock={mockMode}
        />
        <aside className="inspector">
          <div className="panel-heading">
            <span className="eyebrow">INSPECTOR</span>
          </div>
          <h2>
            {unit
              ? unitLocation(unit)
              : selected
                ? locationLabel(selected)
                : 'Select a hex'}
          </h2>
          {hex && (
            <>
              <p className="muted">
                {TERRAIN[hex.terrain].label} · axial {hex.q}, {hex.r}
              </p>
              <div className="inspect-coordinates">
                {hex.label ??
                  (mockMode
                    ? 'Real coordinate grid / synthetic units'
                    : 'Published map grid')}
              </div>
            </>
          )}
          {selected && (
            <>
              <LayerInspector hexId={selected} layer={layers.coverage} />
              <TerrainCoverageInspector hexId={selected} />
            </>
          )}
          {stacks.map((stack) => (
            <StackList
              key={`${selected}:${stack.side}`}
              stack={stack}
              units={view!.units}
              selected={unitId}
              moved={moved}
              onSelect={setUnitId}
            />
          ))}
          {view?.markers
            .filter((marker) => sameLocation(marker.hex, selected))
            .map((marker) => (
              <section key={marker.id}>
                <h3>{marker.label ?? marker.kind}</h3>
                <p className="muted">
                  {marker.kind} · holder: {marker.side ?? 'unheld'}
                </p>
              </section>
            ))}
          {!stacks.length && <p className="empty">No visible stack.</p>}
          {unit && (
            <section className="unit-detail">
              <h3>{unit.name}</h3>
              <StatusBadges unit={unit} />
              {moved.has(unit.id) && (
                <p className="moved-label">Moved this segment</p>
              )}
              <dl>
                <dt>ID</dt>
                <dd>{unit.id}</dd>
                <dt>Kind</dt>
                <dd>{unit.kind}</dd>
                <dt>Nationality</dt>
                <dd>{unit.nationality}</dd>
                <dt>Formation</dt>
                <dd>
                  {unit.parent
                    ? (view?.units[unit.parent]?.name ?? unit.parent)
                    : '—'}
                </dd>
                <dt>Location</dt>
                <dd>{unitLocation(unit)}</dd>
              </dl>
              <dl>
                {Object.entries(unit.detail ?? {}).map(([key, value]) => (
                  <div className="detail-field" key={key}>
                    <dt>{key.replaceAll('_', ' ')}</dt>
                    <dd>
                      {typeof value === 'object'
                        ? JSON.stringify(value)
                        : String(value)}
                    </dd>
                  </div>
                ))}
              </dl>
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
          <button
            onClick={actions.step}
            disabled={!frame || !!state.archiveFrame}
          >
            Step →
          </button>
          <button
            onClick={actions.play}
            disabled={!frame || !!state.archiveFrame}
          >
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
            disabled={!!state.archiveFrame}
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
        {state.archiveFrame && (
          <p className="archive-caption" data-testid="archive-caption">
            Summary checkpoint: intermediate frames are outside retained
            history.{' '}
            <button onClick={() => actions.seek(state.frames[0].seq)}>
              Resume retained history
            </button>
          </p>
        )}
        <StageTimeline
          stages={state.stages}
          onJump={(entry) => {
            actions.seek(entry.frame.seq)
            setEventFilter('all')
            if (entry.hex && HEX_BY_ID.has(entry.hex)) locate(entry.hex)
          }}
        />
        <div className="feed-header">
          <span className="eyebrow">EVENT FEED</span>
          <select
            aria-label="Event filter"
            value={eventFilter}
            onChange={(e) => setEventFilter(e.target.value)}
          >
            <option value="all">All events</option>
            <option value="unit_moved">Movement &amp; notes</option>
            <option value="note">Notes / stops</option>
            <option value="dice_rolled">Dice</option>
            <option value="unit_removed">Removals</option>
            <option value="combat_resolved">Combat</option>
            <option value="phase_changed">Phase</option>
          </select>
          <small>
            {state.frames.length} / 600 frames retained · transcripts bounded at
            1,200
          </small>
        </div>
        <EventFeed frames={events} onLocate={locate} />
      </footer>
    </div>
  )
}
