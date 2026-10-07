import { expect, it } from 'vitest'
import type { UnitView } from './protocol'
import { unitBadges, unitCp } from './status'
const unit = { id: 'u', detail: null } as UnitView
it('displays only published own status and quarter-unit CP, with no defaults for absent detail', () => {
  expect(unitBadges(unit)).toEqual([])
  expect(unitCp(unit)).toBeNull()
  const detailed = {
    ...unit,
    detail: {
      engaged: true,
      combat_pinned: true,
      moved_this_segment: true,
      gun_position: 'deployed',
      reserve: { status: 'first', released_for_cycle: 2 },
      cp_spent_quarters: 5,
    },
  }
  expect(unitBadges(detailed)).toEqual([
    'Engaged',
    'Pinned',
    'Moved this segment',
    'Gun: deployed',
    'Reserve: first',
    'Released cycle 2',
  ])
  expect(unitCp(detailed)).toBe(1.25)
  expect(unitBadges({ ...unit, kind: 'broken_vehicle' })).toEqual([
    'Broken vehicles',
  ])
  expect(
    unitBadges({ ...unit, detail: { reserve: { status: 'none' } } }),
  ).toEqual([])
})
