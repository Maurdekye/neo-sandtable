/** Synthetic stress fixture: no real CNA roster, setup, or rules values. */
import type { Clock, Side, UnitView, ViewState } from '../protocol'
import { HEXES } from '../map/fixture'
export function denseView(clock: Clock): ViewState {
  const cells = HEXES.filter(
    (h) =>
      h.q >= 65 && h.q < 75 && h.r >= 19 && h.r < 27 && h.terrain !== 'sea',
  ).slice(0, 40)
  if (cells.length !== 40)
    throw new Error('Dense fixture requires forty published hexes')
  const units: Record<string, UnitView> = {}
  const kinds = [
    'infantry',
    'recce',
    'artillery',
    'anti_tank',
    'aa',
    'armor',
    'engineers',
  ]
  const stacks = cells.map((cell, index) => {
    const count = index === 0 ? 20 : index < 37 ? 7 : 6
    const side: Side = index < 20 ? 'axis' : 'commonwealth'
    const ids = Array.from({ length: count }, (_, member) => {
      const id = `dense-${index}-${member}`
      units[id] = {
        id,
        side,
        name: `Synthetic ${index + 1}.${member + 1}`,
        kind: member ? kinds[member % kinds.length] : 'hq',
        size: member ? 'battalion' : 'division',
        nationality: side === 'axis' ? 'italian' : 'british',
        hex: cell.id,
        parent: member ? `dense-${index}-0` : `${side}-synthetic-army`,
        detail: {
          strength: 3,
          provenance: 'Synthetic stress fixture; no CNA rules values',
        },
      }
      return id
    })
    return { hex: cell.id, side, unit_ids: ids, visible_count: count }
  })
  for (let i = 0; i < 6; i++) {
    const id = `reserve-${i}`
    units[id] = {
      id,
      side: i < 3 ? 'axis' : 'commonwealth',
      name: `Synthetic reserve ${i + 1}`,
      kind: 'infantry',
      size: 'battalion',
      nationality: 'synthetic',
      hex: null,
      parent: null,
      detail: {
        provenance: 'Synthetic stress fixture',
        location:
          i < 3
            ? { at: 'awaiting_setup', group: 'synthetic-setup' }
            : { at: 'off_map', id: 'box_tripoli' },
      },
    }
  }
  return {
    clock,
    units,
    stacks,
    markers: [
      {
        id: 'dense-dump',
        kind: 'supply_dump',
        hex: cells[0].id,
        side: 'axis',
        label: 'Synthetic supply dump',
      },
    ],
    pending: [
      {
        id: 'dense-setup',
        seat: 'axis.commander',
        kind: 'setup',
        summary: 'Place synthetic reserves',
        opened_seq: 0,
        rules: [],
      },
    ],
  }
}
