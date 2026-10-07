import { Citation } from '../Rules'
import {
  COVERAGE_LAYERS,
  EDGES,
  FEATURE_LAYERS,
  label,
  MAP_LAYERS,
  LAYER_FIXTURE,
  status,
  type Layer,
  type LayerOptions,
} from './layers'
import { HEX_BY_ID } from './fixture'
export function LayerControls({
  options,
  onChange,
}: {
  options: LayerOptions
  onChange: (v: LayerOptions) => void
}) {
  const surveyed = options.coverage
    ? (MAP_LAYERS.coverage.get(options.coverage)?.size ?? 0)
    : 0
  return (
    <details className="layer-controls" open>
      <summary>Map layers</summary>
      {['line:', 'side:'].map((group) => (
        <fieldset key={group}>
          <legend>{group === 'line:' ? 'Routes' : 'Hex boundaries'}</legend>
          {FEATURE_LAYERS.filter((layer) => layer.startsWith(group)).map(
            (layer) => (
              <label key={layer}>
                <input
                  type="checkbox"
                  checked={options.visible.includes(layer)}
                  onChange={(e) =>
                    onChange({
                      ...options,
                      visible: e.target.checked
                        ? [...options.visible, layer]
                        : options.visible.filter((v) => v !== layer),
                    })
                  }
                />
                {label(layer)}
              </label>
            ),
          )}
        </fieldset>
      ))}
      <label>
        Survey coverage{' '}
        <select
          aria-label="Survey coverage"
          value={options.coverage ?? 'off'}
          onChange={(e) =>
            onChange({
              ...options,
              coverage:
                e.target.value === 'off' ? null : (e.target.value as Layer),
            })
          }
        >
          <option value="off">Off</option>
          {COVERAGE_LAYERS.map((layer) => (
            <option key={layer} value={layer}>
              {label(layer)} ({layer.split(':')[0]})
            </option>
          ))}
        </select>
      </label>
      {options.coverage && (
        <p className="muted">
          Hatch = not surveyed for {label(options.coverage)}.{' '}
          {surveyed.toLocaleString()}{' '}
          {options.coverage.includes(':') ? 'edges' : 'cells'} surveyed; absence
          inside this mask is verified.
        </p>
      )}
      <small>
        {MAP_LAYERS.features.length.toLocaleString()}{' '}
        {LAYER_FIXTURE ? 'synthetic' : 'published'} features -{' '}
        {EDGES.length.toLocaleString()} internal boundaries
      </small>
    </details>
  )
}
export function LayerInspector({
  hexId,
  layer,
}: {
  hexId: string
  layer: Layer | null
}) {
  const hex = HEX_BY_ID.get(hexId)
  if (!layer || !hex) return null
  if (!layer.includes(':'))
    return (
      <p className="muted">
        {label(layer)} coverage:{' '}
        {MAP_LAYERS.coverage.get(layer)?.has(hex.id) ? 'surveyed' : 'unknown'}
      </p>
    )
  const adjacent = EDGES.filter((e) => e.a.id === hex.id || e.b.id === hex.id)
  return (
    <details className="edge-inspector">
      <summary>
        {label(layer)} survey at {hex.id}
      </summary>
      {adjacent.map((edge) => (
        <div key={edge.key}>
          <strong>{edge.a.id === hex.id ? edge.b.id : edge.a.id}</strong>
          <span>{status(MAP_LAYERS, layer, edge.key)}</span>
          {MAP_LAYERS.features
            .filter((f) => f.layer === layer && f.edge.key === edge.key)
            .map((f) => (
              <span key={f.layer}>
                {f.highSide && <small>High side: {f.highSide}</small>}
                {f.src
                  .split(';')
                  .filter(Boolean)
                  .map((cite) => (
                    <Citation key={cite} cite={cite} />
                  ))}
              </span>
            ))}
        </div>
      ))}
      {adjacent.length < 6 && (
        <small>
          Map boundary: {6 - adjacent.length} neighbours outside the published
          grid.
        </small>
      )}
    </details>
  )
}
