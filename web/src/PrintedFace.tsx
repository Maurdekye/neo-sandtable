import type { UnitView } from './protocol'
import { printedFace } from './face'
export function PrintedFace({ unit }: { unit: UnitView }) {
  const face = printedFace(unit)
  return (
    <small className="printed-face" data-testid="printed-face">
      Printed face | {unit.size} | {unit.kind} | {unit.nationality}
      {face.counter !== null && <> | Counter: {face.counter}</>}
      {face.stackingPoints !== null && (
        <> | {face.stackingPoints} stacking points</>
      )}
    </small>
  )
}
