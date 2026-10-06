import type { PendingDecision } from './generated/PendingDecision'
/** Read-only decision windows; citations will use the lead's generated contract. */
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
          <small>
            {d.id} · opened #{d.opened_seq}
          </small>
        </article>
      ))}
      {!pending.length && <small>No disclosed decision windows</small>}
    </section>
  )
}
