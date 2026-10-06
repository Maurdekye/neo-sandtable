import type { UnitView } from './protocol'
import { HEX_BY_ID } from './map/fixture'
export function locationLabel(hex: string | null) {
  return hex === null
    ? 'No map position'
    : HEX_BY_ID.has(hex)
      ? hex
      : hex.startsWith('box_')
        ? `Off-map · ${hex}`
        : `Unmapped location · ${hex}`
}

function locationDetail(unit: UnitView) {
  const value = unit.detail?.location
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value
    : undefined
}
export function isOffMap(unit: UnitView) {
  return (
    locationDetail(unit)?.at === 'off_map' ||
    Boolean(unit.hex && !HEX_BY_ID.has(unit.hex))
  )
}
export function unitLocation(unit: UnitView) {
  const detail = locationDetail(unit)
  if (unit.hex) return locationLabel(unit.hex)
  if (detail?.at === 'off_map' && typeof detail.id === 'string')
    return `Off-map · ${detail.id}`
  if (detail?.at === 'awaiting_setup')
    return `Awaiting setup${typeof detail.group === 'string' ? ` · ${detail.group}` : ''}`
  return locationLabel(null)
}
