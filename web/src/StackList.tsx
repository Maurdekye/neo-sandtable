import { useState } from 'react'
import type { UnitView } from './protocol'
import type { Stack } from './generated/Stack'
import { counterSvg } from './map/counters'
export function StackList({
  stack,
  units,
  selected,
  onSelect,
  moved,
}: {
  stack: Stack
  units: Record<string, UnitView>
  selected: string | null
  onSelect: (id: string) => void
  moved: Set<string>
}) {
  const [query, setQuery] = useState(''),
    text = query.trim().toLowerCase()
  const disclosed = stack.unit_ids.flatMap((id) =>
      units[id] ? [units[id]] : [],
    ),
    filtered = disclosed.filter((u) =>
      `${u.name} ${u.id} ${u.kind} ${u.parent ?? ''}`
        .toLowerCase()
        .includes(text),
    )
  return (
    <section>
      <h3>
        {stack.side} · {stack.visible_count ?? 'unknown'} units
      </h3>
      {!stack.unit_ids.length && (
        <p className="muted">Presence disclosed; composition unavailable.</p>
      )}
      {disclosed.length > 8 && (
        <label className="stack-search">
          Find in stack
          <input
            aria-label={`Find unit in ${stack.side} stack`}
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
      )}
      <div className="stack-unit-list">
        {filtered.map((u) => (
          <button
            className={`unit-row ${selected === u.id ? 'selected' : ''} ${moved.has(u.id) ? 'moved' : ''}`}
            key={u.id}
            data-unit-id={u.id}
            onClick={() => onSelect(u.id)}
          >
            <img
              alt={`${u.kind} ${u.size} counter`}
              src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(counterSvg(u))}`}
            />
            <span>
              {u.name}
              <small>
                {u.size} · {u.kind}
                {moved.has(u.id) ? ' - moved' : ''}
              </small>
            </span>
          </button>
        ))}
      </div>
      {text && (
        <small>
          {filtered.length} of {disclosed.length} disclosed units
        </small>
      )}
    </section>
  )
}
