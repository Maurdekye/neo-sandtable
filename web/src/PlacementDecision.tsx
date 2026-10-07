import { useMemo, useState } from 'react'
import type { PendingDecision } from './generated/PendingDecision'
import { HEX_BY_ID } from './map/fixture'
import { locationLabel, unitLocation } from './location'
import type { UnitView } from './protocol'
import { placementContext, placementDestinations } from './setup'
export function PlacementDecision({
  decision,
  active,
  onShow,
  onHex,
  units,
  onUnit,
}: {
  decision: PendingDecision
  active: boolean
  onShow: (id: string) => void
  onHex: (id: string) => void
  units: Record<string, UnitView>
  onUnit: (id: string) => void
}) {
  const context = placementContext(decision),
    unit = context.unit ? units[context.unit] : undefined
  const awaiting = context.group
    ? Object.values(units).filter((u) => {
        const location = u.detail?.location
        return (
          location &&
          typeof location === 'object' &&
          !Array.isArray(location) &&
          location.at === 'awaiting_setup' &&
          location.group === context.group
        )
      })
    : []
  const [search, setSearch] = useState('')
  const destinations = useMemo(
    () => placementDestinations(decision),
    [decision],
  )
  const filtered = useMemo(
    () =>
      destinations?.filter((id) =>
        id.toLowerCase().includes(search.trim().toLowerCase()),
      ) ?? [],
    [destinations, search],
  )
  return (
    <div className="placement-decision">
      {unit ? (
        <button
          className="setup-unit"
          data-unit-id={unit.id}
          onClick={() => onUnit(unit.id)}
        >
          <strong>{unit.name}</strong>
          <small>Placement decision open - {unitLocation(unit)}</small>
        </button>
      ) : decision.kind === 'cna.setup.unit' ? (
        <small>Unit association not supplied by this projection.</small>
      ) : context.dump ? (
        <strong>Dump: {context.dump}</strong>
      ) : null}
      {awaiting.length > 1 && (
        <details className="setup-awaiting">
          <summary>
            {awaiting.length} units awaiting this group's window close
          </summary>
          {awaiting.map((u) => (
            <button
              key={u.id}
              data-unit-id={u.id}
              className={u.id === context.unit ? 'current-placement' : ''}
              onClick={() => onUnit(u.id)}
            >
              {u.name}
            </button>
          ))}
        </details>
      )}

      <p className="muted">
        Blind set-up: positions are published when the shared window closes.
      </p>
      {destinations === null ? (
        <p>Legal destinations not supplied by this projection.</p>
      ) : (
        <>
          <button
            className="show-placement"
            aria-pressed={active}
            onClick={() => onShow(decision.id)}
          >
            {active ? 'Highlighted legal area' : 'Show legal area'} (
            {destinations.length} destinations)
          </button>
          <label>
            Find legal destination
            <input
              aria-label={`Find legal destination for ${decision.id}`}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </label>
          <details className="setup-destinations">
            <summary>Legal destinations ({filtered.length})</summary>
            {filtered.slice(0, 80).map((id) => (
              <button
                key={id}
                disabled={!HEX_BY_ID.has(id)}
                onClick={() => onHex(id)}
              >
                {locationLabel(id)}
              </button>
            ))}
            {filtered.length > 80 && (
              <small>
                Showing first 80; search to find other destinations.
              </small>
            )}
            {!filtered.length && <small>No matching destination.</small>}
          </details>
        </>
      )}
    </div>
  )
}
