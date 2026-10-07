import { Citation } from './Rules'
import { eventHex, eventText } from './events'
import type { Frame } from './stream/model'
import { HEX_BY_ID, TERRAIN } from './map/fixture'
export function EventFeed({
  frames,
  onLocate,
}: {
  frames: Frame[]
  onLocate: (hex: string) => void
}) {
  return (
    <div className="event-feed">
      {frames.map((f) => {
        const e = f.event!,
          hex = eventHex(f),
          known = hex ? HEX_BY_ID.get(hex) : undefined
        return (
          <article
            className="event-row"
            key={f.seq}
            data-kind={e.kind}
            data-hex={hex ?? ''}
          >
            <span>#{f.seq}</span>
            <strong>{e.kind.replaceAll('_', ' ')}</strong>
            <small>{eventText(e)}</small>
            {f.unit_id && (
              <small className="event-unit">Unit: {f.unit_id}</small>
            )}
            {e.kind === 'dice_rolled' && e.rule && <Citation cite={e.rule} />}
            {e.kind === 'combat_resolved' && e.detail && (
              <details>
                <summary>Resolution detail</summary>
                <pre>{JSON.stringify(e.detail, null, 2)}</pre>
              </details>
            )}
            {known ? (
              <button className="event-locate" onClick={() => onLocate(hex!)}>
                Locate {known.id}
              </button>
            ) : (
              <em className="unlocated">
                {hex ? `Off-map: ${hex}` : 'Unlocated'}
              </em>
            )}
            {e.kind === 'unit_moved' && known?.terrain === 'unclassified' && (
              <small className="event-warning">
                Destination terrain unclassified
              </small>
            )}
            {e.kind === 'unit_moved' && known && (
              <small className="event-terrain">
                {TERRAIN[known.terrain].label}
              </small>
            )}
          </article>
        )
      })}
      {!frames.length && <p className="empty">Waiting for campaign events.</p>}
    </div>
  )
}
