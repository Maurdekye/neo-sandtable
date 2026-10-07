import { useId, useState } from 'react'
import { createPortal } from 'react-dom'
import data from './data/rules.json'
import { clockAnchor } from './ruleAnchor'
import type { Clock } from './protocol'
interface Rule {
  title: string
  summary: string
  timing: string[]
}
const registry: Record<string, Rule> = data.rules
export function Citation({ cite }: { cite: string }) {
  const id = useId()
  const [position, setPosition] = useState<{
    left: number
    top: number
  } | null>(null)
  const rule = registry[cite]
  return (
    <>
      <button
        className="citation"
        aria-describedby={position ? id : undefined}
        onMouseEnter={(event) => {
          const r = event.currentTarget.getBoundingClientRect()
          setPosition({
            left: Math.max(8, Math.min(innerWidth - 368, r.left)),
            top: Math.max(8, Math.min(innerHeight - 280, r.bottom + 6)),
          })
        }}
        onFocus={(event) => {
          const r = event.currentTarget.getBoundingClientRect()
          setPosition({
            left: Math.max(8, Math.min(innerWidth - 368, r.left)),
            top: Math.max(8, Math.min(innerHeight - 280, r.bottom + 6)),
          })
        }}
        onMouseLeave={() => setPosition(null)}
        onBlur={() => setPosition(null)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') setPosition(null)
        }}
      >
        {cite}
      </button>
      {position &&
        createPortal(
          <aside
            className="citation-card"
            role="tooltip"
            id={id}
            style={position}
          >
            <small>{cite} · registry paraphrase</small>
            <strong>{rule?.title ?? 'No registry entry'}</strong>
            <p>
              {rule?.summary || 'No summary is recorded for this citation.'}
            </p>
            {Boolean(rule?.timing.length) && (
              <small>{rule.timing.join(' · ')}</small>
            )}
          </aside>,
          document.body,
        )}
    </>
  )
}
export function RulesCoverage({ clock }: { clock?: Clock }) {
  const [scenario, setScenario] = useState('graziani')
  const coverage: Record<string, typeof data.coverage.graziani> = data.coverage
  const report = coverage[scenario]
  const anchor = clock ? clockAnchor(clock) : ''
  return (
    <details className="rules-coverage">
      <summary>
        Rules coverage · {report.implemented}/{report.applicable}
      </summary>
      <label>
        Registry scenario{' '}
        <select
          aria-label="Coverage scenario"
          value={scenario}
          onChange={(event) => setScenario(event.target.value)}
        >
          {Object.keys(coverage).map((key) => (
            <option key={key}>{key}</option>
          ))}
        </select>
      </label>
      <p className="muted">
        Build-time engine citations; conditional cases included. Counts do not
        guarantee execution in this campaign's profile.
      </p>
      <small>
        {report.tested} tested · {report.unsupported} unsupported ·{' '}
        {report.missing} missing
      </small>
      <table>
        <thead>
          <tr>
            <th>Sequence step</th>
            <th>Implemented / applicable</th>
          </tr>
        </thead>
        <tbody>
          {report.by_anchor.map((row) => (
            <tr
              key={row.anchor}
              className={row.anchor === anchor ? 'current-step' : ''}
            >
              <td>{row.anchor.replaceAll('_', ' ')}</td>
              <td
                title={`${row.tested} tested; ${row.unsupported} unsupported; ${row.missing} missing`}
              >
                {row.implemented} / {row.applicable}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <small>
        Registry {data.registry_sha256.slice(0, 8)} · engine{' '}
        {data.engine_sha256.slice(0, 8)} · {data.warning_count} report warnings
      </small>
    </details>
  )
}
