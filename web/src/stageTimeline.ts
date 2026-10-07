import { explanation } from './commentary'
import type { Clock } from './protocol'
import type { Frame } from './stream/model'
import { eventHex, eventText } from './events'
import { unitLocation } from './location'
export const MAX_STAGES = 12,
  MAX_STAGE_ENTRIES = 80,
  MAX_STAGE_FORMATIONS = 128
export type SummaryCategory =
  | 'combat'
  | 'removals'
  | 'breakdown'
  | 'supply'
  | 'arrivals'
  | 'counters'
  | 'notes'
  | 'decisions'
export interface SummaryEntry {
  id: string
  category: SummaryCategory
  label: string
  frame: Frame
  hex: string | null
  explanation?: string
}
export interface FormationMoves {
  id: string
  label: string
  count: number
  longest: number
  entry: SummaryEntry
}
export interface StageSummary {
  key: string
  clock: Clock
  firstSeq: number
  lastSeq: number
  partial: boolean
  moves: number
  combats: number
  formations: FormationMoves[]
  entries: SummaryEntry[]
  counts: Partial<Record<SummaryCategory, number>>
  droppedEntries: number
  ungroupedMoves: number
}
export function stageKey(clock: Clock) {
  return `${clock.game_turn}:${clock.op_stage ?? 'pre'}`
}
export function stageTitle(clock: Clock) {
  return `GT ${clock.game_turn} / ${clock.op_stage == null ? 'before OpStages' : `OpStage ${clock.op_stage}`}`
}
function noteCategory(text: string): SummaryCategory {
  // These headings classify received prose, not inferred state or numerical losses.
  if (/arrival|withdraw|reinforc/i.test(text)) return 'arrivals'
  if (/breakdown|broken vehicle/i.test(text)) return 'breakdown'
  if (/shortage|ration|suppl|stores|water|ammo|fuel/i.test(text))
    return 'supply'
  if (/barrage|assault|engage|combat|retreat|gun position/i.test(text))
    return 'combat'
  return 'notes'
}
function notable(frame: Frame, previous?: Frame): SummaryEntry | null {
  const e = frame.event
  if (!e) return null
  let category: SummaryCategory,
    label = eventText(e)
  switch (e.kind) {
    case 'combat_resolved':
      category = 'combat'
      break
    case 'unit_removed':
      category = !e.reason
        ? 'counters'
        : /withdraw/i.test(e.reason)
          ? 'arrivals'
          : 'removals'
      break
    case 'dice_rolled':
      category = /breakdown/i.test(e.purpose)
        ? 'breakdown'
        : /barrage|assault|combat/i.test(e.purpose)
          ? 'combat'
          : 'notes'
      break
    case 'note':
      category = noteCategory(e.text)
      break
    case 'decision_resolved':
      category = 'decisions'
      label = `${e.seat}: ${e.summary}`
      break
    case 'stack_updated':
      category = 'counters'
      break
    case 'stack_removed':
      category = 'counters'
      break
    case 'unit_updated': {
      const old = previous?.view.units[e.unit.id]
      if (e.unit.kind === 'broken_vehicle') {
        category = 'breakdown'
        label = `Breakdown counter updated: ${e.unit.name}`
      } else if (old && !old.hex && e.unit.hex) {
        category = 'arrivals'
        label = `${e.unit.name}: appeared on map (${unitLocation(old)})`
      } else if (old?.hex && !e.unit.hex) {
        category = 'arrivals'
        label = `${e.unit.name}: moved off map (${unitLocation(e.unit)})`
      } else if (
        (!old && e.unit.hex) ||
        (old?.hex && e.unit.hex && old.hex !== e.unit.hex)
      ) {
        category = 'counters'
        label = `${e.unit.name}: counter visible at ${e.unit.hex}`
      } else return null
      break
    }
    default:
      return null
  }
  return {
    id: `event:${frame.seq}`,
    category,
    label,
    frame,
    hex: eventHex(frame),
    explanation: explanation(frame.event),
  }
}
/** Immutable, bounded summaries retain exact representative frames independently of the playback ring. */
export function recordStage(
  stages: StageSummary[],
  frame: Frame,
  previous?: Frame,
): StageSummary[] {
  const key = stageKey(frame.view.clock),
    old = stages.at(-1),
    same = old?.key === key
  const stage: StageSummary = same
    ? {
        ...old!,
        lastSeq: frame.seq,
        formations: [...old!.formations],
        entries: [...old!.entries],
        counts: { ...old!.counts },
      }
    : {
        key,
        clock: frame.view.clock,
        firstSeq: frame.seq,
        lastSeq: frame.seq,
        partial: frame.event === null && frame.seq > 0,
        moves: 0,
        combats: 0,
        formations: [],
        entries: [],
        counts: {},
        droppedEntries: 0,
        ungroupedMoves: 0,
      }
  const e = frame.event
  if (e?.kind === 'combat_resolved') stage.combats++
  if (e?.kind === 'unit_moved') {
    stage.moves++
    const unit = previous?.view.units[e.unit_id] ?? frame.view.units[e.unit_id]
    const id = unit?.parent ?? e.unit_id
    const label =
      (unit?.parent
        ? (previous?.view.units[id]?.name ?? frame.view.units[id]?.name)
        : unit?.name) ?? id
    const entry: SummaryEntry = {
      id: `event:${frame.seq}`,
      category: 'notes',
      label: eventText(e),
      frame,
      hex: eventHex(frame),
    }
    const index = stage.formations.findIndex((f) => f.id === id)
    if (index >= 0) {
      const before = stage.formations[index]
      stage.formations[index] = {
        ...before,
        count: before.count + 1,
        ...(e.path.length >= before.longest
          ? { longest: e.path.length, entry }
          : {}),
      }
    } else if (stage.formations.length < MAX_STAGE_FORMATIONS)
      stage.formations.push({
        id,
        label,
        count: 1,
        longest: e.path.length,
        entry,
      })
    else stage.ungroupedMoves++
  }
  const entry = notable(frame, previous)
  if (entry) {
    stage.counts[entry.category] = (stage.counts[entry.category] ?? 0) + 1
    stage.entries.push(entry)
    if (stage.entries.length > MAX_STAGE_ENTRIES) {
      stage.entries.shift()
      stage.droppedEntries++
    }
  }
  return [...(same ? stages.slice(0, -1) : stages), stage].slice(-MAX_STAGES)
}
export function summaryFrame(
  stages: StageSummary[],
  seq: number,
): Frame | undefined {
  for (const stage of stages) {
    const entry =
      stage.entries.find((e) => e.frame.seq === seq) ??
      stage.formations.find((f) => f.entry.frame.seq === seq)?.entry
    if (entry) return entry.frame
  }
}
