import { PrintedFace } from '../PrintedFace'
import { isOpponent } from '../face'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Board } from '../map/Board'
import { DEFAULT_LAYERS } from '../map/layers'
import { HEX_BY_ID, INITIAL_HEX } from '../map/fixture'
import { StatusBadges } from '../StatusBadges'
import { Transcripts } from '../Transcripts'
import { EventFeed } from '../EventFeed'
import { initialState, receive, type ViewerState } from '../stream/model'
import { createSocketStream, streamUrl } from '../stream/socket'
import type { Perspective } from '../protocol'
import { DecisionForm } from './DecisionForm'
import type { MapPick } from './ActionField'
import type { SeatState } from './client'
import type { Boot } from './boot'
export function Console({ boot }: { boot: Boot }) {
  const [identity, setIdentity] = useState<{
      campaign: string
      seat: string
      perspective: Perspective
    } | null>(null),
    [seatState, setSeatState] = useState<SeatState | null>(null),
    [state, setState] = useState<ViewerState | null>(null),
    [error, setError] = useState(''),
    [status, setStatus] = useState('Checking seat capability...'),
    [lost, setLost] = useState(false),
    [active, setActive] = useState(''),
    [selected, setSelected] = useState(INITIAL_HEX),
    [pick, setPick] = useState<MapPick | null>(null),
    [inspection, setInspection] = useState<unknown>(null),
    [log, setLog] = useState<string[]>([]),
    [focus, setFocus] = useState<{
      hex: string
      nonce: number
      bounds?: string[]
    } | null>(null)
  const epoch = useRef<number | null>(null),
    viewer = useRef<ViewerState | null>(null),
    mounted = useRef(false),
    polling = useRef<Promise<void> | null>(null),
    inspectionSerial = useRef(0),
    pickStamp = useRef('')
  const onLog = useCallback(
    (text: string) => setLog((old) => [...old, text].slice(-100)),
    [],
  )
  const refresh = useCallback((): Promise<void> => {
    if (polling.current) return polling.current
    polling.current = (async () => {
      try {
        const next = await boot.client.observe()
        if (!mounted.current) return
        if (epoch.current === null) {
          if (next.controller !== 'human') {
            setLost(true)
            setError('This seat is not bound to a human controller.')
            return
          }
          epoch.current = next.epoch
        } else if (
          epoch.current !== next.epoch ||
          next.controller !== 'human'
        ) {
          setLost(true)
          setPick(null)
          setError(
            'You have lost control of this seat. Open a new launcher link after a human handover.',
          )
          return
        }
        setSeatState(next)
        setError('')
      } catch (e) {
        if (mounted.current)
          setError(e instanceof Error ? e.message : 'Cannot refresh this seat')
      } finally {
        polling.current = null
      }
    })()
    return polling.current
  }, [boot])
  useEffect(() => {
    mounted.current = true
    let close = () => {},
      timer: ReturnType<typeof setTimeout> | null = null
    const poll = async () => {
      await refresh()
      if (mounted.current) timer = setTimeout(() => void poll(), 3000)
    }
    void boot.client
      .session()
      .then((session) => {
        if (!mounted.current) return
        setIdentity(session)
        viewer.current = initialState(session.perspective)
        const transport = createSocketStream({
          url: streamUrl(boot.server, session.campaign, boot.token),
          lastGoodSeq: () => viewer.current?.lastSeq ?? null,
          status: (next) => {
            if (mounted.current) setStatus(next.message)
          },
          deliver: (message) => {
            if (!mounted.current || !viewer.current) return
            if (
              (message.type === 'hello' &&
                (message.perspective !== session.perspective ||
                  message.campaign.id !== session.campaign)) ||
              (message.type === 'transcript' && message.seat !== session.seat)
            ) {
              setLost(true)
              setError('Server sent a different seat scope.')
              transport.close()
              return
            }
            if (message.type === 'snapshot' && !viewer.current.frames.length) {
              const hex = message.view.stacks.find(
                (stack) =>
                  stack.side === session.seat.split('.')[0] &&
                  HEX_BY_ID.has(stack.hex),
              )?.hex
              if (hex) {
                setSelected(hex)
                setFocus({ hex, nonce: Date.now() })
              }
            }
            const next = receive(viewer.current, message)
            viewer.current = next.state
            setState(next.state)
            if (next.subscribe) transport.subscribe(next.subscribe)
            if (
              message.type === 'event' &&
              ['decision_opened', 'decision_resolved', 'seat_paused'].includes(
                message.event.kind,
              )
            )
              void refresh()
          },
        })
        close = transport.close
        transport.subscribe({
          type: 'subscribe',
          perspective: session.perspective,
          from_seq: null,
        })
        void poll()
      })
      .catch((e) => {
        if (mounted.current)
          setError(e instanceof Error ? e.message : 'Cannot open seat access')
      })
    return () => {
      mounted.current = false
      if (timer) clearTimeout(timer)
      close()
      boot.client.close()
    }
  }, [boot, refresh])
  const pending = seatState?.pending ?? [],
    request = pending.find((d) => d.id === active) ?? pending[0],
    view = state?.frames.at(-1)?.view
  const stamp = request ? `${request.id}:${request.revision}` : ''
  const disabled =
    lost ||
    !identity ||
    !seatState ||
    !!error ||
    seatState.paused ||
    !!seatState.failure ||
    state?.connection !== 'live'
  const onPick = useCallback(
    (p: MapPick | null) => {
      pickStamp.current = stamp
      setPick(p)
      const target = p?.values?.find((id) => HEX_BY_ID.has(id))
      if (target && p?.kind !== 'unit')
        setFocus({
          hex: target,
          nonce: Date.now(),
          bounds: p!.values!.filter((id) => HEX_BY_ID.has(id)),
        })
    },
    [stamp],
  )
  const inspect = async (target: string) => {
    const serial = ++inspectionSerial.current
    const disclosed = viewer.current?.frames.at(-1)?.view.units[target]?.hex
    if (disclosed && HEX_BY_ID.has(disclosed)) {
      setSelected(disclosed)
      setFocus({ hex: disclosed, nonce: Date.now() })
    }
    try {
      const result = await boot.client.inspect(target)
      if (mounted.current && serial === inspectionSerial.current)
        setInspection(result)
    } catch (e) {
      if (mounted.current && serial === inspectionSerial.current)
        setInspection({
          error: e instanceof Error ? e.message : 'Inspection failed',
        })
    }
  }
  const legalHexes =
    pick?.values?.flatMap((id) =>
      pick.kind === 'unit'
        ? view?.units[id]?.hex && HEX_BY_ID.has(view.units[id].hex)
          ? [view.units[id].hex]
          : []
        : HEX_BY_ID.has(id)
          ? [id]
          : [],
    ) ?? []
  const pickUnit = (id: string) => {
    void inspect(id)
    if (
      !disabled &&
      pick &&
      pickStamp.current === stamp &&
      pick.kind === 'unit' &&
      pick.values?.includes(id)
    )
      pick.accept(id)
  }
  const selectHex = (hex: string) => {
    setSelected(hex)
    if (
      pick &&
      pickStamp.current === stamp &&
      !disabled &&
      pick.kind !== 'unit'
    ) {
      const permitted =
        pick.values?.find(
          (id) => HEX_BY_ID.get(id)?.id === HEX_BY_ID.get(hex)?.id,
        ) ?? (pick.values === null ? hex : null)
      if (permitted) pick.accept(permitted)
    }
  }
  const seatInfo =
    state?.campaign?.seats.filter((s) => s.id === identity?.seat) ?? []
  return (
    <main className="human-console">
      <header className="console-header">
        <div>
          <span className="eyebrow">HUMAN SEAT</span>
          <h1>{identity?.seat ?? 'Seat console'}</h1>
        </div>
        <div>
          <strong>{status}</strong>
          <small>
            Seat-only view / capability kept in memory / epoch{' '}
            {epoch.current ?? 'not established'}
          </small>
        </div>
      </header>
      {error && (
        <p className="console-error" role="alert">
          {error}
        </p>
      )}
      {seatState?.paused && (
        <p role="alert">Seat paused. {seatState.failure}</p>
      )}
      <div className="console-workspace">
        <section className="console-map">
          <Board
            view={view}
            selected={selected}
            focus={focus}
            onSelect={selectHex}
            layers={DEFAULT_LAYERS}
            frames={state?.frames ?? []}
            seq={state?.lastSeq ?? null}
            scope={identity?.perspective ?? 'seat:unattached'}
            moving={true}
            allowBatch={false}
            moved={
              new Set(
                view
                  ? Object.values(view.units)
                      .filter((u) => u.detail?.moved_this_segment === true)
                      .map((u) => u.id)
                  : [],
              )
            }
            placement={
              pick && legalHexes.length
                ? {
                    label: `${pick.label}: enumerated targets`,
                    hexes: legalHexes,
                    targetLabel: 'enumerated target hexes',
                  }
                : null
            }
            terrainCoverage={true}
          />
          <div className="console-picking">
            {pick ? (
              <>
                <strong>Picking {pick.label}</strong>
                <span>
                  {pick.values
                    ? `${pick.values.length} enumerated choices`
                    : 'Candidate map picks; legality requires engine preflight'}
                </span>
                <button onClick={() => onPick(null)}>Stop picking</button>
              </>
            ) : (
              <span>Select a hex to inspect its disclosed counters.</span>
            )}
          </div>
          <div className="console-units">
            {view?.stacks
              .filter(
                (s) => HEX_BY_ID.get(s.hex)?.id === HEX_BY_ID.get(selected)?.id,
              )
              .map((s) => (
                <section key={`${s.side}:${s.hex}`}>
                  <strong>
                    {s.side}: {s.visible_count ?? 'count not reported'}{' '}
                    disclosed counters / units
                  </strong>
                  {s.unit_ids.map(
                    (id) =>
                      view.units[id] && (
                        <button
                          key={id}
                          onClick={() => pickUnit(id)}
                          disabled={
                            !!pick &&
                            (pick.kind !== 'unit' || !pick.values?.includes(id))
                          }
                        >
                          {view.units[id].name}
                          {identity &&
                          isOpponent(
                            view.units[id].side,
                            identity.perspective,
                          ) ? (
                            <PrintedFace unit={view.units[id]} />
                          ) : (
                            <StatusBadges unit={view.units[id]} />
                          )}
                        </button>
                      ),
                  )}
                </section>
              ))}
          </div>
          <details>
            <summary>Seat inspection and previews</summary>
            <pre>{JSON.stringify(inspection, null, 2)}</pre>
          </details>
        </section>
        <aside className="console-orders">
          <h2>Pending decisions</h2>
          <nav>
            {pending.map((d) => (
              <button
                disabled={disabled}
                className={request?.id === d.id ? 'selected' : ''}
                key={d.id}
                onClick={() => {
                  setActive(d.id)
                  onPick(null)
                }}
              >
                {d.summary}
              </button>
            ))}
          </nav>
          {request && epoch.current !== null ? (
            <DecisionForm
              key={request.id}
              request={request}
              client={boot.client}
              epoch={epoch.current}
              disabled={disabled}
              onRefresh={refresh}
              onPick={onPick}
              onLog={onLog}
              onInspect={(target) => void inspect(target)}
            />
          ) : (
            <p>
              {identity
                ? 'No pending decisions for this seat.'
                : 'Waiting for seat authorization.'}
            </p>
          )}
          <details open>
            <summary>Console log</summary>
            {log.map((line, i) => (
              <p key={i}>{line}</p>
            ))}
          </details>
          <details>
            <summary>Own seat event feed</summary>
            <EventFeed
              frames={
                state?.frames
                  .filter((f) => f.event)
                  .slice(-50)
                  .reverse() ?? []
              }
              onLocate={(hex) => {
                setSelected(hex)
                setFocus({ hex, nonce: Date.now() })
              }}
            />
          </details>
          <Transcripts
            seats={seatInfo}
            messages={
              state?.transcripts.filter((m) => m.seat === identity?.seat) ?? []
            }
            commentaries={state?.commentaries ?? []}
          />
        </aside>
      </div>
    </main>
  )
}
