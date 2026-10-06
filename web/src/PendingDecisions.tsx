import type { PendingDecision } from './generated/PendingDecision'
/** Read-only, perspective-projected decision windows and server rule citations. */
export function PendingDecisions({ pending }: { pending: PendingDecision[] }) {
  return (
    <section className="pending-decisions">
      <h3>Pending decisions ({pending.length})</h3>
      {pending.map((d) => (
        <article key={d.id}>
          <strong>{d.summary}</strong>
          <small>
            {d.seat} · {d.kind}
          </small>
          <div className="rule-citations" aria-label="Rule citations">
            {d.rules?.map((rule) => (
              <span key={rule}>{rule}</span>
            ))}
          </div>
          <small>
            {d.id} · opened #{d.opened_seq}
          </small>
        </article>
      ))}
      {!pending.length && <small>No disclosed decision windows</small>}
    </section>
  )
}
