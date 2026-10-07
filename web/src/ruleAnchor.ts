import type { Clock } from './protocol'
export function clockAnchor(clock: Clock): string {
  return [
    clock.stage,
    clock.phase !== clock.stage ? clock.phase : null,
    clock.segment,
    clock.step,
  ]
    .filter(Boolean)
    .join('.')
}
