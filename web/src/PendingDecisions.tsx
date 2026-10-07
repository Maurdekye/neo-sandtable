import { PlacementDecision } from './PlacementDecision'
import { isPlacement, placementContext } from './setup'
import { movementUnits } from './movement'
import type { UnitView } from './protocol'
import { Citation } from './Rules'
import type { PendingDecision } from './generated/PendingDecision'
/** Read-only, perspective-projected decision windows and server rule citations. */
export function PendingDecisions({
  pending,
  units,
  onUnit,
  activePlacement,
  onPlacement,
  onHex,
}: {
  pending: PendingDecision[]
  units: Record<string, UnitView>
  onUnit: (id: string) => void
  activePlacement: string | undefined
  onPlacement: (id: string) => void
  onHex: (id: string) => void
}) {
  const awaiting = Object.values(units).filter((unit) => {
    const location = unit.detail?.location
    return (
      location &&
      typeof location === 'object' &&
      !Array.isArray(location) &&
      location.at === 'awaiting_setup'
    )
  })
  return (
    <section className="pending-decisions">
      <h3>Pending decisions ({pending.length})</h3>
      {awaiting.length > 0 && (
        <details className="setup-awaiting all-awaiting">
          <summary>{awaiting.length} units awaiting set-up</summary>
          {awaiting.map((unit) => {
            const request = pending.find(
              (d) =>
                d.kind === 'cna.setup.unit' &&
                placementContext(d).unit === unit.id,
            )
            return (
              <div key={unit.id}>
                <button
                  data-unit-id={unit.id}
                  className={request ? 'current-placement' : ''}
                  onClick={() => onUnit(unit.id)}
                >
                  {unit.name}
                </button>
                {request ? (
                  <button onClick={() => onPlacement(request.id)}>
                    Show placement options
                  </button>
                ) : (
                  <small>
                    No open placement window supplied for this unit.
                  </small>
                )}
              </div>
            )
          })}
        </details>
      )}
      {pending.map((d) => (
        <article
          className={
            isPlacement(d) && activePlacement === d.id
              ? 'active-placement'
              : undefined
          }
          key={d.id}
          data-seat={d.seat}
          data-decision-id={d.id}
        >
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
          {isPlacement(d) && (
            <PlacementDecision
              decision={d}
              active={activePlacement === d.id}
              onShow={onPlacement}
              onHex={onHex}
              units={units}
              onUnit={onUnit}
            />
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
