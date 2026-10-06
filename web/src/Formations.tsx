import { useState } from 'react'
import type { UnitView } from './protocol'
import { formationTree, filterTree, type FormationNode } from './hierarchy'
import { unitLocation, isOffMap } from './location'
function Branch({
  node,
  depth,
  search,
  onUnit,
}: {
  node: FormationNode
  depth: number
  search: boolean
  onUnit: (id: string) => void
}) {
  const button = node.unit && (
    <button
      className="formation-unit"
      onClick={() => onUnit(node.unit!.id)}
      title={node.unit.name}
    >
      {node.unit.name}
      <small>{unitLocation(node.unit)}</small>
    </button>
  )
  if (!node.children.length) return <div className="oa-leaf">{button}</div>
  return (
    <details className="oa-branch" open={search || depth === 0}>
      <summary title={node.unit?.name ?? node.id}>
        {node.unit?.name ?? node.id}
        <small>{node.count} units</small>
      </summary>
      {button}
      <div className="oa-children">
        {node.children.map((child) => (
          <Branch
            key={child.id}
            node={child}
            depth={depth + 1}
            search={search}
            onUnit={onUnit}
          />
        ))}
      </div>
    </details>
  )
}
export function Formations({
  units,
  onUnit,
}: {
  units: Record<string, UnitView>
  onUnit: (id: string) => void
}) {
  const [query, setQuery] = useState(''),
    list = Object.values(units)
  const unplaced = list.filter((u) => u.hex === null && !isOffMap(u)),
    offmap = list.filter(isOffMap)
  return (
    <>
      <label className="formation-search">
        Find formation or unit
        <input
          aria-label="Find formation or unit"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          type="search"
        />
      </label>
      {['axis', 'commonwealth'].map((side) => {
        const own = list.filter((u) => u.side === side),
          tree = filterTree(formationTree(own), query)
        return (
          <div className="formation" key={side}>
            <h3>
              <i className={`dot ${side}`} />
              {side}
              <small>{own.length}</small>
            </h3>
            {tree.map((node) => (
              <Branch
                key={node.id}
                node={node}
                depth={0}
                search={Boolean(query.trim())}
                onUnit={onUnit}
              />
            ))}
            {!tree.length && <small>No matching disclosed units</small>}
          </div>
        )
      })}
      {(
        [
          ['Awaiting setup', unplaced],
          ['Off-map / unmapped', offmap],
        ] as [string, UnitView[]][]
      ).map(([label, members]) => {
        const group = members
        return (
          group.length > 0 && (
            <details className="location-group" key={label}>
              <summary>
                {label} ({group.length})
              </summary>
              {group.map((unit) => (
                <button key={unit.id} onClick={() => onUnit(unit.id)}>
                  {unit.name}
                  <small>{unitLocation(unit)}</small>
                </button>
              ))}
            </details>
          )
        )
      })}
    </>
  )
}
