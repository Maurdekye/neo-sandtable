import { useEffect, useState } from 'react'
import type { Access } from './access'
import { apiRequest } from './access'
import type { ViewerState } from './stream/model'
import type { SeatInfo } from './protocol'
import { decodeMessage } from './stream/wire'
import {
  decodeObservation,
  observedWait,
  receivedRate,
  type SeatObservation,
} from './monitoring'
interface Polled {
  seats: SeatInfo[]
  observations: Record<string, SeatObservation>
  handovers: Record<string, number>
  error: string
  at: number
}
const emptyPoll = (): Polled => ({
  seats: [],
  observations: {},
  handovers: {},
  error: '',
  at: 0,
})
export function SeatMonitor({
  state,
  access,
}: {
  state: ViewerState
  access?: Access
}) {
  const [now, setNow] = useState(() => Date.now())
  const [polled, setPolled] = useState<Polled>(emptyPoll)
  const campaign = state.campaign
  const operator = state.perspective === 'operator'
  const live = state.cursor === null
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [])
  useEffect(() => {
    if (!access || !access.session.operator || !operator || !live || !campaign)
      return
    const abort = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    let previous = emptyPoll()
    async function refresh() {
      try {
        const root = `/api/campaigns/${encodeURIComponent(campaign!.id)}/seats`
        const list = await apiRequest(access!, `${root}?perspective=operator`, {
          signal: abort.signal,
        })
        const hello = decodeMessage(
          JSON.stringify({
            type: 'hello',
            protocol: 1,
            perspective: 'operator',
            campaign: { ...campaign, seats: list },
          }),
        )
        if (hello?.type !== 'hello') throw new Error('Invalid seat list')
        const observations: Record<string, SeatObservation> = {}
        // Serial bounded polling avoids rebuilding ten seat observations simultaneously.
        for (const seat of hello.campaign.seats.slice(0, 128)) {
          const data = await apiRequest(
            access!,
            `${root}/${encodeURIComponent(seat.id)}/observe`,
            { signal: abort.signal },
          )
          observations[seat.id] = decodeObservation(data)
        }
        if (abort.signal.aborted) return
        const handovers = { ...previous.handovers }
        for (const [id, value] of Object.entries(observations)) {
          if (
            previous.observations[id] &&
            previous.observations[id].epoch !== value.epoch
          )
            handovers[id] = value.epoch
        }
        previous = {
          seats: hello.campaign.seats,
          observations,
          handovers,
          error: '',
          at: Date.now(),
        }
        setPolled(previous)
      } catch (error) {
        if (!abort.signal.aborted)
          setPolled({ ...emptyPoll(), error: String(error) })
      }
      if (!abort.signal.aborted) timer = setTimeout(() => void refresh(), 10000)
    }
    void refresh()
    return () => {
      abort.abort()
      clearTimeout(timer)
      setPolled(emptyPoll())
    }
  }, [access, campaign, operator, live])
  if (!operator || !campaign) return null
  if (!live)
    return (
      <div className="monitor-history">
        Seat monitoring resumes in Live; current binding state is not
        historical.
      </div>
    )
  const rate = receivedRate(state.monitoring, now)
  const seats = polled.at ? polled.seats : campaign.seats
  const clock = state.frames.at(-1)?.view.clock
  return (
    <section className="seat-monitor" aria-label="AI play monitoring">
      <div className="monitor-progress">
        <strong>
          GT {clock?.game_turn ?? '-'} / OpStage {clock?.op_stage ?? '-'} /{' '}
          {clock?.phase.replaceAll('_', ' ') ?? 'Connecting'}
        </strong>
        <span data-testid="answer-count">
          {state.monitoring.answered} answers received
        </span>
        <span>
          {rate === null
            ? 'Collecting recent rate'
            : `${rate.toFixed(1)} received answers/min`}
        </span>
        <small>
          Since snapshot; replay included. Waiting duration starts when
          observed.
        </small>
        {access && (
          <small>
            {polled.at
              ? `Status polled ${Math.max(0, Math.floor((now - polled.at) / 1000))}s ago`
              : 'Seat status not refreshed'}
            {polled.error && `: ${polled.error}`}
          </small>
        )}
      </div>
      <div className="seat-strip">
        {seats.map((seat) => {
          const pending =
            state.frames
              .at(-1)
              ?.view.pending.filter((d) => d.seat === seat.id) ?? []
          const obs = polled.observations[seat.id]
          const record = state.monitoring.seats[seat.id]
          const status =
            obs?.paused || seat.status === 'paused'
              ? 'paused'
              : seat.status === 'failed'
                ? 'failed'
                : pending.length
                  ? 'awaiting answer'
                  : seat.status
          return (
            <article
              className={`seat-card seat-${status.replaceAll(' ', '-')}`}
              key={seat.id}
              data-seat={seat.id}
            >
              <div className="seat-card-heading">
                <strong>{seat.id}</strong>
                <span>{status}</span>
              </div>
              <p className="seat-controller">
                {seat.controller?.label ?? 'No controller reported'}
              </p>
              <small>Provider / model: not separately reported</small>
              {pending.length ? (
                <p className="seat-wait">
                  {pending[0].kind} /{' '}
                  {observedWait(
                    state.monitoring.pendingSince[pending[0].id],
                    now,
                  )}
                  {pending.length > 1 && ` (+${pending.length - 1})`}
                </p>
              ) : (
                <p className="seat-wait">No pending decision received</p>
              )}
              <small>
                {seat.controller?.kind === 'scripted'
                  ? 'scripted, no usage'
                  : 'Tokens: not reported / USD: not reported'}
              </small>
              {record?.summary && (
                <p className="seat-answer" title={record.summary}>
                  Accepted: {record.summary}
                </p>
              )}
              {record?.explanation && (
                <p className="seat-answer" title={record.explanation}>
                  Why: {record.explanation}
                </p>
              )}
              {(obs?.failure || record?.toolError) && (
                <p className="monitor-error seat-answer">
                  {obs?.failure ?? `Tool error: ${record?.toolError}`}
                </p>
              )}
              {(record || obs || pending.length > 0) && (
                <details>
                  <summary>Latest answer & activity</summary>
                  {record?.summary ? (
                    <>
                      <p>
                        <strong>Accepted:</strong> {record.summary}
                      </p>
                      {record.explanation && (
                        <blockquote>{record.explanation}</blockquote>
                      )}
                    </>
                  ) : (
                    <p>Accepted answer not received</p>
                  )}
                  {pending.map((d) => (
                    <p key={d.id}>{d.summary}</p>
                  ))}
                  {record?.activity && (
                    <small>Latest transcript: {record.activity}</small>
                  )}
                  {record?.toolError && (
                    <p className="monitor-error">
                      Last tool result failed: {record.toolError}
                    </p>
                  )}
                  {obs?.failure && (
                    <p className="monitor-error">Pause reason: {obs.failure}</p>
                  )}
                  {obs && (
                    <small>
                      Controller epoch {obs.epoch}
                      {polled.handovers[seat.id] !== undefined &&
                        ' / handover observed'}
                    </small>
                  )}
                </details>
              )}
            </article>
          )
        })}
      </div>
    </section>
  )
}
