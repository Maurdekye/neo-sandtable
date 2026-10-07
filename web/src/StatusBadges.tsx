import type { UnitView } from './protocol'
import { unitBadges, unitCp } from './status'
export function StatusBadges({ unit }: { unit: UnitView }) {
  const badges = unitBadges(unit),
    cp = unitCp(unit)
  return (
    <span className="status-badges" data-testid="unit-status">
      {badges.map((b) => (
        <span key={b}>{b}</span>
      ))}
      {cp !== null && <span>{cp} CP spent</span>}
    </span>
  )
}
