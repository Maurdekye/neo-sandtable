import type { Schema, Value } from './schema'
import { concrete, defaultDraft, fieldKind, nullable } from './schema'
export interface MapPick {
  label: string
  kind: 'hex' | 'unit' | 'path'
  values: string[] | null
  from?: string
  accept: (value: string) => void
}
export function ActionField({
  schema,
  value,
  label = 'Action',
  onChange,
  onPick,
  onInspect,
  disabled = false,
}: {
  schema: Schema
  value: Value
  label?: string
  onChange: (v: Value) => void
  onPick: (pick: MapPick) => void
  onInspect?: (target: string) => void
  disabled?: boolean
}) {
  const s = concrete(schema),
    kind = fieldKind(schema),
    description = schema.description ?? s.description
  if (nullable(schema))
    return (
      <fieldset disabled={disabled}>
        <legend>{label}</legend>
        <label>
          <input
            type="checkbox"
            checked={value === null}
            onChange={(e) =>
              onChange(e.target.checked ? null : defaultDraft(s))
            }
          />
          {label === 'Action' ? 'Pass' : 'Omit'}
        </label>
        {value !== null && (
          <ActionField
            schema={s}
            value={value}
            label={label}
            onChange={onChange}
            onPick={onPick}
            onInspect={onInspect}
            disabled={disabled}
          />
        )}
        {description && <small>{description}</small>}
      </fieldset>
    )
  if (s.type === 'object') {
    const record =
      typeof value === 'object' && value !== null && !Array.isArray(value)
        ? value
        : {}
    return (
      <fieldset disabled={disabled}>
        <legend>{label}</legend>
        {Object.entries(s.properties!).map(([key, child]) => (
          <ActionField
            key={key}
            schema={child}
            value={record[key] ?? defaultDraft(child)}
            label={key}
            disabled={disabled}
            onPick={onPick}
            onInspect={onInspect}
            onChange={(v) =>
              onChange(
                Object.fromEntries([
                  ...Object.entries(record).filter(([k]) => k !== key),
                  ...(v === null && !s.required!.includes(key)
                    ? []
                    : [[key, v] as const]),
                ]),
              )
            }
          />
        ))}
      </fieldset>
    )
  }
  if (s.type === 'array') {
    const list = Array.isArray(value) ? value : []
    const max = Math.min(s.maxItems ?? 1000, 1000)
    const replace = (i: number, v: Value) =>
      onChange(list.map((x, j) => (i === j ? v : x)))
    const swap = (i: number, j: number) =>
      onChange(list.map((x, k) => (k === i ? list[j] : k === j ? list[i] : x)))
    return (
      <fieldset disabled={disabled}>
        <legend>{label}</legend>
        {description && <small>{description}</small>}
        {list.map((v, i) => (
          <div className="console-list-item" key={i}>
            <ActionField
              schema={s.items!}
              value={v}
              label={`${label} ${i + 1}`}
              onChange={(x) => replace(i, x)}
              onPick={onPick}
              onInspect={onInspect}
              disabled={disabled}
            />
            <div>
              <button
                disabled={disabled || i === 0}
                onClick={() => swap(i, i - 1)}
              >
                Up
              </button>
              <button
                disabled={disabled || i === list.length - 1}
                onClick={() => swap(i, i + 1)}
              >
                Down
              </button>
              <button
                disabled={disabled}
                onClick={() => onChange(list.filter((_, j) => i !== j))}
              >
                Remove
              </button>
            </div>
          </div>
        ))}
        <button
          disabled={disabled || list.length >= max}
          onClick={() => onChange([...list, defaultDraft(s.items!)])}
        >
          Add {label}
        </button>
        {kind === 'path' && (
          <button
            disabled={disabled || list.length >= max}
            onClick={() =>
              onPick({
                label,
                kind: 'path',
                values: s.items?.enum ?? null,
                from: schema['x-from'] ?? s['x-from'],
                accept: (hex) => onChange([...list, hex]),
              })
            }
          >
            Append hex on map
          </button>
        )}
        <small>
          {list.length} / {s.maxItems ?? 'unbounded'} items; minimum{' '}
          {s.minItems ?? 0}
        </small>
      </fieldset>
    )
  }
  return (
    <div className="console-field">
      <label>
        {label}
        {s.enum ? (
          <select
            disabled={disabled}
            aria-label={label}
            value={typeof value === 'string' ? value : ''}
            onChange={(e) => onChange(e.target.value)}
          >
            <option value="" disabled>
              Choose...
            </option>
            {s.enum.map((id, i) => (
              <option key={`${id}-${i}`} value={id}>
                {schema['x-options']?.find((o) => o.id === id)?.label ??
                  s['x-options']?.find((o) => o.id === id)?.label ??
                  id}
              </option>
            ))}
          </select>
        ) : s.type === 'boolean' ? (
          <select
            disabled={disabled}
            aria-label={label}
            value={String(value)}
            onChange={(e) => onChange(e.target.value === 'true')}
          >
            <option value="false">False</option>
            <option value="true">True</option>
          </select>
        ) : (
          <input
            disabled={disabled}
            aria-label={label}
            type={s.type === 'integer' ? 'number' : 'text'}
            min={s.minimum}
            max={s.maximum}
            value={
              typeof value === 'string' || typeof value === 'number'
                ? value
                : ''
            }
            onChange={(e) =>
              onChange(
                s.type === 'integer'
                  ? e.target.value === ''
                    ? ''
                    : Number(e.target.value)
                  : e.target.value,
              )
            }
          />
        )}
      </label>
      {s.enum
        ?.filter((id) => ['pass', 'done'].includes(id))
        .map((id) => (
          <button key={id} disabled={disabled} onClick={() => onChange(id)}>
            {id === 'done' ? 'Done' : 'Pass'}
          </button>
        ))}
      {description && <small>{description}</small>}
      {s.enum &&
        (schema['x-options'] ?? s['x-options'])?.find((o) => o.id === value)
          ?.detail && (
          <small>
            {
              (schema['x-options'] ?? s['x-options'])!.find(
                (o) => o.id === value,
              )!.detail
            }
          </small>
        )}
      {kind === 'unit' &&
        typeof value === 'string' &&
        s.enum?.includes(value) &&
        onInspect && (
          <button disabled={disabled} onClick={() => onInspect(value)}>
            Inspect selected unit (CP / fuel)
          </button>
        )}
      {(kind === 'hex' || kind === 'unit') && (
        <button
          disabled={disabled}
          onClick={() =>
            onPick({ label, kind, values: s.enum ?? null, accept: onChange })
          }
        >
          Pick {kind} on map
        </button>
      )}
      {s.minLength !== undefined && (
        <small>
          {s.minLength} to {s.maxLength ?? 'unbounded'} characters
        </small>
      )}
    </div>
  )
}
