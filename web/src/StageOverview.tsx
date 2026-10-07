import { HEX_BY_ID } from './map/fixture'
import {
  stageTitle,
  MAX_STAGES,
  MAX_STAGE_ENTRIES,
  type StageSummary,
  type SummaryEntry,
  type SummaryCategory,
} from './stageTimeline'
const labels: Record<SummaryCategory, string> = {
  combat: 'Combat results / notes',
  removals: 'Losses / removals',
  breakdown: 'Breakdown dice / counters / notes',
  supply: 'Supply / shortage notes',
  arrivals: 'Arrivals / off-map changes / withdrawals',
  counters: 'Counter and stack changes',
  notes: 'Other notes',
  decisions: 'Seat decisions',
}
export function StageTimeline({
  stages,
  onJump,
}: {
  stages: StageSummary[]
  onJump: (entry: SummaryEntry) => void
}) {
  return (
    <details className="stage-overview" data-testid="stage-overview">
      <summary>
        Stage overview <small>{stages.length} received stage summaries</small>
      </summary>
      <div className="stage-scroll">
        <p className="muted">
          Current perspective only. Latest {MAX_STAGES} stages; up to{' '}
          {MAX_STAGE_ENTRIES} notable entries per stage. Counts describe
          received events. Note headings do not establish losses or shortages.
        </p>
        {[...stages].reverse().map((stage, i) => (
          <details className="stage-summary" key={stage.key} open={i === 0}>
            <summary>
              {stageTitle(stage.clock)}{' '}
              <small>
                {stage.moves} moves | {stage.combats} combat results |{' '}
                {stage.counts.removals ?? 0} removals |{' '}
                {stage.counts.supply ?? 0} supply notes | #{stage.firstSeq}-
                {stage.lastSeq}
              </small>
            </summary>
            {stage.partial && (
              <p className="muted">
                Partial: opened from snapshot #{stage.firstSeq}; earlier events
                are not included.
              </p>
            )}
            <div className="stage-columns">
              <section>
                <h3>Movement by received formation or counter</h3>
                {!stage.formations.length && (
                  <p className="muted">No movement events received.</p>
                )}
                {stage.formations.map((f) => (
                  <button
                    className="stage-entry stage-move"
                    key={f.id}
                    data-seq={f.entry.frame.seq}
                    onClick={() => onJump(f.entry)}
                  >
                    <strong>{f.label}</strong>
                    <span>
                      {f.count} moves; largest {f.longest} disclosed route hexes
                    </span>
                    <small>
                      {f.entry.frame.event?.kind === 'unit_moved'
                        ? f.entry.frame.event.path.join(' / ')
                        : ''}{' '}
                      |{' '}
                      {f.entry.hex && HEX_BY_ID.has(f.entry.hex)
                        ? `Locate ${f.entry.hex}`
                        : 'Unlocated'}{' '}
                      | #{f.entry.frame.seq}
                    </small>
                  </button>
                ))}
                {stage.ungroupedMoves > 0 && (
                  <p>
                    {stage.ungroupedMoves} further received moves outside
                    retained formation grouping.
                  </p>
                )}
              </section>
              <section>
                <h3>Notable events</h3>
                {stage.droppedEntries > 0 && (
                  <p className="muted">
                    {stage.droppedEntries} older entry details omitted; event
                    counts remain cumulative.
                  </p>
                )}
                {(Object.keys(labels) as SummaryCategory[])
                  .filter((c) => stage.counts[c])
                  .map((category) => (
                    <details className="stage-category" key={category}>
                      <summary>
                        {labels[category]} ({stage.counts[category]} received)
                      </summary>
                      {[...stage.entries]
                        .reverse()
                        .filter((e) => e.category === category)
                        .map((e) => (
                          <button
                            className="stage-entry"
                            key={e.id}
                            data-category={e.category}
                            data-seq={e.frame.seq}
                            onClick={() => onJump(e)}
                          >
                            <small>
                              #{e.frame.seq}
                              {e.hex && HEX_BY_ID.has(e.hex)
                                ? ` | Locate ${e.hex}`
                                : ' | Unlocated'}
                            </small>
                            <span>{e.label}</span>
                            {e.explanation && (
                              <span className="decision-explanation">
                                Seat commentary: {e.explanation}
                              </span>
                            )}
                          </button>
                        ))}
                      {!stage.entries.some((e) => e.category === category) && (
                        <p className="muted">
                          Details outside retained summary history.
                        </p>
                      )}
                    </details>
                  ))}
              </section>
            </div>
          </details>
        ))}
        {!stages.length && (
          <p className="empty">Waiting for received events.</p>
        )}
      </div>
    </details>
  )
}
