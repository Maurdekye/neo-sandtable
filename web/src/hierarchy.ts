import type { UnitView } from './protocol'
export interface FormationNode {
  id: string
  unit?: UnitView
  children: FormationNode[]
  count: number
}
/** Only projected units and their declared parents participate; unknown parents keep their ids. */
export function formationTree(units: UnitView[]): FormationNode[] {
  const nodes = new Map<string, FormationNode>(),
    parents = new Map<string, string | null>()
  for (const unit of units) {
    nodes.set(unit.id, { id: unit.id, unit, children: [], count: 1 })
    parents.set(unit.id, unit.parent)
  }
  for (const unit of units)
    if (unit.parent && !nodes.has(unit.parent)) {
      nodes.set(unit.parent, { id: unit.parent, children: [], count: 0 })
      parents.set(unit.parent, null)
    }
  // A malformed cycle must not hide units or recurse forever. Leave one member unlinked.
  for (const id of [...nodes.keys()].sort()) {
    const seen = new Set([id])
    let parent = parents.get(id)
    while (parent) {
      if (seen.has(parent)) {
        parents.set(id, null)
        break
      }
      seen.add(parent)
      parent = parents.get(parent)
    }
  }
  const roots: FormationNode[] = []
  for (const [id, node] of nodes) {
    const parent = parents.get(id)
    if (parent) nodes.get(parent)!.children.push(node)
    else roots.push(node)
  }
  function count(node: FormationNode): number {
    node.children.sort((a, b) =>
      (a.unit?.name ?? a.id).localeCompare(b.unit?.name ?? b.id),
    )
    node.count =
      (node.unit ? 1 : 0) + node.children.reduce((n, c) => n + count(c), 0)
    return node.count
  }
  roots.forEach(count)
  return roots.sort((a, b) =>
    (a.unit?.name ?? a.id).localeCompare(b.unit?.name ?? b.id),
  )
}
export function filterTree(
  nodes: FormationNode[],
  query: string,
): FormationNode[] {
  const text = query.trim().toLowerCase()
  if (!text) return nodes
  return nodes.flatMap((node) => {
    if (`${node.id} ${node.unit?.name ?? ''}`.toLowerCase().includes(text))
      return [node]
    const children = filterTree(node.children, text)
    return children.length ? [{ ...node, children }] : []
  })
}
