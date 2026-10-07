import type { PendingDecision } from './generated/PendingDecision'
import type { JsonValue } from './generated/serde_json/JsonValue'
import { HEX_BY_ID } from './map/fixture'
export function record(
  value: JsonValue | undefined,
): Record<string, JsonValue> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value
    : undefined
}
export function isPlacement(decision: PendingDecision) {
  return (
    decision.kind === 'cna.setup.unit' || decision.kind === 'cna.setup.dump'
  )
}
/** Choices come exclusively from the authorized decision projection; no geographic domain is guessed. */
export function placementDestinations(
  decision: PendingDecision,
): string[] | null {
  if (!isPlacement(decision)) return null
  const schema = record(decision.space)
  if (
    schema?.type !== 'string' ||
    !Array.isArray(schema.enum) ||
    !schema.enum.every((id) => typeof id === 'string')
  )
    return null
  return [...new Set(schema.enum as string[])]
}
export function placementHexes(
  decision: PendingDecision | undefined,
): string[] {
  return [
    ...new Set(
      (decision ? (placementDestinations(decision) ?? []) : []).flatMap(
        (id) => {
          const hex = HEX_BY_ID.get(id)
          return hex ? [hex.id] : []
        },
      ),
    ),
  ]
}

/** Lead-owned ActionSpace.context is projected as x-context; absence is explicit, never parsed from prose. */
export function placementContext(decision: PendingDecision): {
  unit?: string
  group?: string
  dump?: string
  pool?: string
} {
  if (!decision.kind.startsWith('cna.setup.')) return {}
  const context = record(record(decision.space)?.['x-context'])
  return Object.fromEntries(
    ['unit', 'group', 'dump', 'pool'].flatMap((key) =>
      typeof context?.[key] === 'string' ? [[key, context[key]]] : [],
    ),
  )
}
