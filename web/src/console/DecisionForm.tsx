import { useCallback, useEffect, useRef, useState } from 'react'
import { Citation } from '../Rules'
import { ActionField, type MapPick } from './ActionField'
import type { SeatClient, SeatRequest } from './client'
import { ConsoleError } from './client'
import {
  defaultDraft,
  passOnly,
  reconcileDraft,
  validateDraft,
  type Value,
} from './schema'
export function DecisionForm({
  request,
  client,
  epoch,
  disabled,
  onRefresh,
  onPick,
  onInspect,
  onLog,
}: {
  request: SeatRequest
  client: SeatClient
  epoch: number
  disabled: boolean
  onRefresh: () => Promise<void>
  onPick: (p: MapPick | null) => void
  onInspect: (target: string) => void
  onLog: (text: string) => void
}) {
  const [version, setVersion] = useState(
    JSON.stringify(request.schema) + request.revision,
  )
  const [draft, setDraft] = useState<Value>(() => defaultDraft(request.schema)),
    [explanation, setExplanation] = useState(''),
    [busy, setBusy] = useState(false),
    [message, setMessage] = useState(''),
    [auto, setAuto] = useState(false)
  const attempt = useRef<{ fingerprint: string; key: string } | null>(null),
    autoAttempt = useRef(''),
    alive = useRef(true),
    inFlight = useRef(false),
    latest = useRef({ disabled, stamp: '' })
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])
  const current = JSON.stringify(request.schema) + request.revision
  if (version !== current) {
    setVersion(current)
    setDraft(reconcileDraft(request.schema, draft))
    setMessage(
      'The decision changed. Valid draft fields were retained; review and validate again.',
    )
  }
  latest.current = {
    disabled,
    stamp: JSON.stringify([request.id, request.revision, epoch, current]),
  }
  const context = request.schema['x-context']
  const contextUnit =
    context &&
    typeof context === 'object' &&
    'unit' in context &&
    typeof context.unit === 'string'
      ? context.unit
      : null
  const errors = validateDraft(request.schema, draft),
    locked = disabled || busy
  const run = useCallback(
    async (submit: boolean) => {
      if (
        disabled ||
        busy ||
        inFlight.current ||
        validateDraft(request.schema, draft).length ||
        [...explanation].length > 2000
      )
        return
      inFlight.current = true
      const stamp = latest.current.stamp
      setBusy(true)
      setMessage('Validating with the engine...')
      onPick(null)
      try {
        const result = await client.validate(
          request.id,
          draft,
          epoch,
          request.revision,
        )
        if (
          typeof result !== 'object' ||
          result === null ||
          !('valid' in result) ||
          result.valid !== true
        )
          throw new Error('Engine did not approve the draft')
        if (
          !alive.current ||
          latest.current.disabled ||
          latest.current.stamp !== stamp
        )
          throw new Error(
            'Control or decision changed during preflight; submit cancelled.',
          )
        if (!submit) {
          setMessage('Engine preflight passed.')
          return
        }
        const fingerprint = JSON.stringify([
          request.id,
          request.revision,
          epoch,
          draft,
          explanation.trim(),
        ])
        if (attempt.current?.fingerprint !== fingerprint)
          attempt.current = { fingerprint, key: crypto.randomUUID() }
        await client.submit(
          request,
          draft,
          epoch,
          attempt.current.key,
          explanation,
        )
        onLog(
          `${request.id}: answer accepted${passOnly(request.schema) && auto ? ' (automatic pass)' : ''}`,
        )
        setMessage('Answer accepted.')
        await onRefresh()
      } catch (error) {
        setMessage(
          error instanceof Error ? error.message : 'Seat request failed',
        )
        if (error instanceof ConsoleError && [404, 409].includes(error.status))
          await onRefresh()
      } finally {
        inFlight.current = false
        setBusy(false)
      }
    },
    [
      disabled,
      busy,
      request,
      draft,
      explanation,
      client,
      epoch,
      onPick,
      onLog,
      onRefresh,
      auto,
    ],
  )
  useEffect(() => {
    const stamp = `${request.id}:${request.revision}:${epoch}`
    if (
      auto &&
      passOnly(request.schema) &&
      !locked &&
      autoAttempt.current !== stamp
    ) {
      autoAttempt.current = stamp
      void run(true)
    }
  }, [auto, locked, request, epoch, run])
  return (
    <section className="console-decision">
      <h2>{request.summary}</h2>
      <small>
        {request.kind} / revision {request.revision}
      </small>
      <div className="rules">
        {request.rules.map((cite) => (
          <Citation key={cite} cite={cite} />
        ))}
      </div>
      {request.schema['x-context'] !== undefined && (
        <details>
          <summary>Decision context</summary>
          <pre>{JSON.stringify(request.schema['x-context'], null, 2)}</pre>
        </details>
      )}
      {contextUnit && (
        <button disabled={locked} onClick={() => onInspect(contextUnit)}>
          Inspect {contextUnit} (CP / fuel)
        </button>
      )}
      <ActionField
        schema={request.schema}
        value={draft}
        disabled={locked}
        onChange={(v) => {
          setDraft(v)
          setMessage('')
          onPick(null)
        }}
        onPick={onPick}
        onInspect={onInspect}
      />
      <label>
        Optional seat commentary
        <textarea
          disabled={locked}
          aria-label="Seat commentary"
          value={explanation}
          onChange={(e) => setExplanation(e.target.value)}
        />
        <small>
          {[...explanation].length} / 2000 characters; visible to this seat, its
          side and the operator.
        </small>
      </label>
      {errors.map((error) => (
        <p className="console-error" key={error}>
          {error}
        </p>
      ))}
      {[...explanation].length > 2000 && (
        <p className="console-error">Commentary exceeds 2000 characters.</p>
      )}
      <div className="console-submit">
        <button
          disabled={locked || errors.length > 0}
          onClick={() => void run(false)}
        >
          Validate draft
        </button>
        <button
          className="primary"
          disabled={
            locked || errors.length > 0 || [...explanation].length > 2000
          }
          onClick={() => void run(true)}
        >
          Validate and submit
        </button>
      </div>
      {passOnly(request.schema) && (
        <label>
          <input
            type="checkbox"
            disabled={locked}
            checked={auto}
            onChange={(e) => setAuto(e.target.checked)}
          />
          Automatically answer this pass-only decision
        </label>
      )}
      <p role="status">{message}</p>
    </section>
  )
}
