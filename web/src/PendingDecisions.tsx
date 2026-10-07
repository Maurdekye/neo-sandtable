import { movementUnits } from './movement'
import type { UnitView } from './protocol'
import { Citation } from './Rules'
import type { PendingDecision } from './generated/PendingDecision'
/** Read-only, perspective-projected decision windows and server rule citations. */
export function PendingDecisions({
  pending,
  units,
  onUnit,
}: {
  pending: PendingDecision[]
  units: Record<string, UnitView>
  onUnit: (id: string) => void
}) {
  return (
    <section className="pending-decisions">
      <h3>Pending decisions ({pending.length})</h3>
      {pending.map((d) => (
        <article key={d.id} data-seat={d.seat} data-decision-id={d.id}>
          <strong>{d.summary}</strong>
          <small>
            {d.seat} · {d.kind}
          </small>
          {d.kind === 'cna.movement.orders' && (
            <details className="movable-units" open>
              <summary>
                Units still eligible to move ({movementUnits(d)?.length ?? '?'})
              </summary>
              {movementUnits(d) === null ? (
                <p>Eligibility not supplied by this projection.</p>
              ) : (
                movementUnits(d)!.map((id) => (
                  <button
                    key={id}
                    disabled={!units[id]}
                    onClick={() => onUnit(id)}
                  >
                    {units[id]?.name ?? id}
                    <small>{units[id]?.hex ?? 'Location unavailable'}</small>
                  </button>
                ))
              )}
            </details>
          )}
          <div className="rule-citations" aria-label="Rule citations">
            {d.rules?.map((rule) => (
              <Citation key={rule} cite={rule} />
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
