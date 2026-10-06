import { useEffect, useRef, useState } from 'react'
import type { SeatInfo, TranscriptMessage } from './protocol'
import { pairTranscripts } from './stream/transcripts'
const filters: Record<string, string[]> = {
  all: [],
  assistant: ['assistant_text'],
  reasoning: ['reasoning'],
  tools: ['tool_call', 'tool_result'],
  decisions: ['decision_submitted'],
  'System 1': ['system1_query', 'system1_answer'],
  system: ['system'],
}
function body(m: TranscriptMessage) {
  const e = m.entry
  switch (e.kind) {
    case 'assistant_text':
    case 'reasoning':
    case 'system':
      return e.text
    case 'tool_call':
      return `${e.tool}(${JSON.stringify(e.args)})`
    case 'tool_result':
      return `${e.ok ? 'OK' : 'FAILED'} · ${e.summary}`
    case 'decision_submitted':
      return `${e.decision_id} · ${e.summary}`
    case 'system1_query':
      return `${e.question}
${e.options.join(' / ')}`
    case 'system1_answer':
      return `${e.choice}
${JSON.stringify(e.scores)}`
  }
}
function SeatTranscript({
  seat,
  messages,
}: {
  seat: SeatInfo
  messages: TranscriptMessage[]
}) {
  const [filter, setFilter] = useState('all'),
    [follow, setFollow] = useState(true)
  const scroller = useRef<HTMLDivElement>(null)
  const rows = pairTranscripts(
    messages.filter(
      (m) =>
        m.seat === seat.id &&
        (!filters[filter].length || filters[filter].includes(m.entry.kind)),
    ),
  )
  useEffect(() => {
    if (follow && scroller.current)
      scroller.current.scrollTop = scroller.current.scrollHeight
  }, [messages, filter, follow])
  return (
    <>
      <div className="transcript-controls">
        <span>{seat.controller?.label}</span>
        <select
          aria-label="Transcript filter"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        >
          {Object.keys(filters).map((k) => (
            <option key={k}>{k}</option>
          ))}
        </select>
        <button onClick={() => setFollow(!follow)} aria-pressed={follow}>
          {follow ? 'Following ↓' : 'Follow live ↓'}
        </button>
      </div>
      <div
        className="transcript-scroll"
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget
          setFollow(el.scrollHeight - el.clientHeight - el.scrollTop < 35)
        }}
      >
        {rows.map(({ message, result }) => (
          <article
            className={`entry entry-${message.entry.kind}`}
            key={String(message.tseq)}
          >
            <div className="entry-meta">
              <span>{message.entry.kind.replaceAll('_', ' ')}</span>
              <time>
                {new Date(message.at).toLocaleTimeString([], { hour12: false })}
              </time>
              <small>#{String(message.game_seq)}</small>
            </div>
            <p>{body(message)}</p>
            {result && (
              <div className="tool-result">
                {body(result)}
                <details>
                  <summary>Result detail</summary>
                  <pre>
                    {JSON.stringify(
                      result.entry.kind === 'tool_result'
                        ? result.entry.detail
                        : null,
                      null,
                      2,
                    )}
                  </pre>
                </details>
              </div>
            )}
            {message.entry.kind === 'tool_call' && !result && (
              <small className="muted">
                Awaiting result / outside retained history
              </small>
            )}
          </article>
        ))}
        {!rows.length && (
          <p className="empty">No entries in this frame and filter.</p>
        )}
      </div>
    </>
  )
}
export function Transcripts({
  seats,
  messages,
}: {
  seats: SeatInfo[]
  messages: TranscriptMessage[]
}) {
  const [active, setActive] = useState('axis.commander')
  const seat = seats.find((s) => s.id === active) ?? seats[0]
  return (
    <section className="sessions">
      <div className="panel-heading">
        <span className="eyebrow">AGENT SESSIONS</span>
        <span className="pill">MOCK TRANSCRIPTS</span>
      </div>
      <div className="seat-tabs" role="tablist" aria-label="AI seats">
        {seats.map((s) => (
          <button
            role="tab"
            aria-selected={s.id === seat?.id}
            key={s.id}
            onClick={() => setActive(s.id)}
          >
            <i className={`dot ${s.side}`} />
            {s.id.replace('.', ' · ').replaceAll('_', ' ')}
          </button>
        ))}
      </div>
      {seat ? (
        <SeatTranscript key={seat.id} seat={seat} messages={messages} />
      ) : (
        <p className="empty">Waiting for authorized seats.</p>
      )}
    </section>
  )
}
