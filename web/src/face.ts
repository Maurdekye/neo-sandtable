import type { Perspective, Side, UnitView } from './protocol'
/** Printed fields received in UnitView; never derive contents or unknown values. */
export function printedFace(unit: UnitView) {
  const d = unit.detail ?? {}
  return {
    counter: typeof d.counter === 'string' ? d.counter : null,
    stackingPoints:
      typeof d.stacking_points === 'number' &&
      Number.isFinite(d.stacking_points)
        ? d.stacking_points
        : null,
  }
}
export function isOpponent(side: Side, perspective: Perspective): boolean {
  return (
    perspective !== 'operator' &&
    perspective !== `side:${side}` &&
    !perspective.startsWith(`seat:${side}.`)
  )
}
