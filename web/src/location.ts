import { HEX_BY_ID } from './map/fixture'
export function locationLabel(hex: string | null) {
  return hex === null
    ? 'Awaiting setup'
    : HEX_BY_ID.has(hex)
      ? hex
      : hex.startsWith('box_')
        ? `Off-map · ${hex}`
        : `Unmapped location · ${hex}`
}
