import type { UnitView } from './protocol'
export function unitBadges(unit: UnitView): string[] {
  const d = unit.detail ?? {},
    badges: string[] = []
  if (d.engaged === true) badges.push('Engaged')
  if (d.combat_pinned === true) badges.push('Pinned')
  if (d.moved_this_segment === true) badges.push('Moved this segment')
  if (typeof d.gun_position === 'string')
    badges.push(`Gun: ${d.gun_position.replaceAll('_', ' ')}`)
  const r = d.reserve
  if (r && typeof r === 'object' && !Array.isArray(r)) {
    if (typeof r.status === 'string' && r.status !== 'none')
      badges.push(`Reserve: ${r.status.replaceAll('_', ' ')}`)
    if (typeof r.released_for_cycle === 'number')
      badges.push(`Released cycle ${r.released_for_cycle}`)
    if (r.offensive_assault_used === true) badges.push('Reserve assault used')
    if (r.extra_dp_applied === true) badges.push('Extra DP applied')
  }
  if (unit.kind === 'broken_vehicle') badges.push('Broken vehicles')
  return badges
}
export function unitCp(unit: UnitView): number | null {
  const d = unit.detail ?? {}
  if (
    typeof d.cp_spent_quarters === 'number' &&
    Number.isFinite(d.cp_spent_quarters)
  )
    return d.cp_spent_quarters / 4
  return typeof d.cp_spent === 'number' && Number.isFinite(d.cp_spent)
    ? d.cp_spent
    : null
}
