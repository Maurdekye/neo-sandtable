import type { UnitView } from '../protocol'
export const SYMBOLS: Record<string, string> = {
  broken_vehicle: '<path d="M20 16L40 28M40 16L20 28"/><path d="M20 32H40"/>',
  infantry: '<path d="M18 15L42 29M42 15L18 29"/>',
  armor: '<ellipse cx="30" cy="22" rx="9" ry="5"/>',
  artillery: '<circle cx="30" cy="22" r="3" fill="currentColor"/>',
  recce: '<path d="M18 29L42 15"/>',
  hq: '<text x="30" y="26" text-anchor="middle" font-size="10">HQ</text>',
  engineers: '<path d="M20 28V17H40V28M25 17V25M35 17V25"/>',
  anti_tank: '<path d="M20 28L30 17L40 28Z"/>',
  aa: '<path d="M20 28Q30 10 40 28"/>',
  truck: '<circle cx="25" cy="25" r="2"/><circle cx="35" cy="25" r="2"/>',
  motorized:
    '<path d="M18 15L42 29M42 15L18 29"/><circle cx="25" cy="32" r="1.5"/><circle cx="35" cy="32" r="1.5"/>',
  mechanized:
    '<path d="M18 15L42 29M42 15L18 29"/><ellipse cx="30" cy="22" rx="9" ry="5"/>',
}
function escape(value: string) {
  return value.replace(
    /[&<>"']/g,
    (c) =>
      ({
        '&': '&amp;',
        '<': '&lt;',
        '>': '&gt;',
        '"': '&quot;',
        "'": '&apos;',
      })[c]!,
  )
}
export function counterSvg(unit: UnitView): string {
  const bg = unit.side === 'axis' ? '#dbc8a0' : '#8ebbbb'
  const echelon =
    (
      {
        company: 'I',
        battalion: 'II',
        regiment: 'III',
        brigade: 'X',
        division: 'XX',
      } as Record<string, string>
    )[unit.size] ?? '?'
  const accent =
    unit.nationality === 'italian'
      ? '#567553'
      : unit.nationality === 'british'
        ? '#a25144'
        : '#426b8c'
  return `<svg xmlns="http://www.w3.org/2000/svg" width="60" height="48" viewBox="0 0 60 48"><rect x="1" y="1" width="58" height="46" rx="3" fill="${bg}" stroke="#142a30" stroke-width="2"/><rect x="3" y="3" width="5" height="42" fill="${accent}"/><g color="#142a30" fill="none" stroke="currentColor" stroke-width="1.6"><rect x="18" y="15" width="24" height="14"/>${SYMBOLS[unit.kind] ?? '<text x="30" y="25" text-anchor="middle">?</text>'}</g><g fill="#142a30" font-family="sans-serif" text-anchor="middle"><text x="30" y="12" font-size="9">${echelon}</text><text x="32" y="42" font-size="10">${escape(String(unit.detail?.strength ?? '?'))}</text></g></svg>`
}
