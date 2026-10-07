export type Value =
  null | boolean | number | string | Value[] | { [key: string]: Value }
export type Schema = {
  type?: 'null' | 'string' | 'integer' | 'boolean' | 'object' | 'array'
  anyOf?: Schema[]
  enum?: string[]
  description?: string
  minimum?: number
  maximum?: number
  minLength?: number
  maxLength?: number
  pattern?: string
  minItems?: number
  maxItems?: number
  items?: Schema
  properties?: Record<string, Schema>
  required?: string[]
  additionalProperties?: false
  'x-kind'?: string
  'x-from'?: string
  'x-options'?: { id: string; label: string; detail?: string | null }[]
  'x-context'?: unknown
}
const own = (v: object, k: string) => Object.prototype.hasOwnProperty.call(v, k)
const object = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v)
const safe = (n: unknown) =>
  typeof n === 'number' && Number.isSafeInteger(n) && n >= 0
// Fail closed on schema constraints that this renderer cannot validate.
export function decodeSchema(value: unknown): Schema {
  let nodes = 0
  const read = (v: unknown, depth: number): Schema => {
    if (!object(v) || depth > 20 || ++nodes > 20000)
      throw new Error('Unsupported action schema')
    const allowed = new Set([
      'type',
      'anyOf',
      'enum',
      'description',
      'minimum',
      'maximum',
      'minLength',
      'maxLength',
      'pattern',
      'minItems',
      'maxItems',
      'items',
      'properties',
      'required',
      'additionalProperties',
    ])
    if (Object.keys(v).some((k) => !allowed.has(k) && !k.startsWith('x-')))
      throw new Error('Unsupported schema constraint')
    if (v.description !== undefined && typeof v.description !== 'string')
      throw new Error('Invalid schema description')
    const s: Schema = { description: v.description as string | undefined }
    for (const k of ['x-kind', 'x-from'] as const) {
      if (v[k] !== undefined && typeof v[k] !== 'string')
        throw new Error('Invalid schema annotation')
      if (v[k] !== undefined) s[k] = v[k] as string
    }
    if (v['x-context'] !== undefined) s['x-context'] = v['x-context']
    if (v['x-options'] !== undefined) {
      if (
        !Array.isArray(v['x-options']) ||
        !v['x-options'].every(
          (o) =>
            object(o) &&
            typeof o.id === 'string' &&
            typeof o.label === 'string' &&
            (o.detail == null || typeof o.detail === 'string'),
        )
      )
        throw new Error('Invalid choice labels')
      s['x-options'] = v['x-options'] as Schema['x-options']
    }
    if (v.anyOf !== undefined) {
      if (
        !Array.isArray(v.anyOf) ||
        v.anyOf.length !== 2 ||
        v.type !== undefined
      )
        throw new Error('Unsupported schema alternatives')
      if (
        Object.keys(v).some(
          (k) => !['anyOf', 'description'].includes(k) && !k.startsWith('x-'),
        )
      )
        throw new Error('Unsupported nullable constraint')
      s.anyOf = v.anyOf.map((x) => read(x, depth + 1))
      if (s.anyOf.filter((x) => x.type === 'null').length !== 1)
        throw new Error('Only nullable alternatives are supported')
      return s
    }
    if (
      !['null', 'string', 'integer', 'boolean', 'object', 'array'].includes(
        String(v.type),
      )
    )
      throw new Error('Unknown action field type')
    s.type = v.type as Schema['type']
    const byType: Record<string, string[]> = {
      null: [],
      string: ['enum', 'pattern', 'minLength', 'maxLength'],
      integer: ['minimum', 'maximum'],
      boolean: [],
      object: ['properties', 'required', 'additionalProperties'],
      array: ['items', 'minItems', 'maxItems'],
    }
    if (
      Object.keys(v).some(
        (k) =>
          !['type', 'description'].includes(k) &&
          !k.startsWith('x-') &&
          !byType[s.type!].includes(k),
      )
    )
      throw new Error('Unsupported field constraint')
    for (const k of [
      'minLength',
      'maxLength',
      'minItems',
      'maxItems',
    ] as const) {
      if (v[k] !== undefined && !safe(v[k]))
        throw new Error('Invalid schema bound')
      if (v[k] !== undefined) s[k] = v[k] as number
    }
    for (const k of ['minimum', 'maximum'] as const) {
      if (
        v[k] !== undefined &&
        !(typeof v[k] === 'number' && Number.isSafeInteger(v[k]))
      )
        throw new Error('Invalid integer bound')
      if (v[k] !== undefined) s[k] = v[k] as number
    }
    if (
      (s.minimum ?? -Infinity) > (s.maximum ?? Infinity) ||
      (s.minLength ?? 0) > (s.maxLength ?? Infinity) ||
      (s.minItems ?? 0) > (s.maxItems ?? Infinity)
    )
      throw new Error('Inverted schema bounds')
    if (v.enum !== undefined) {
      if (
        s.type !== 'string' ||
        !Array.isArray(v.enum) ||
        v.enum.length > 20000 ||
        !v.enum.every((x) => typeof x === 'string')
      )
        throw new Error('Invalid enum')
      s.enum = v.enum
    }
    if (v.pattern !== undefined) {
      if (v.pattern !== '^[A-E][0-9]{4}$')
        throw new Error('Unsupported string pattern')
      s.pattern = v.pattern
    }
    if (s.type === 'object') {
      if (
        !object(v.properties) ||
        Object.keys(v.properties).length > 1000 ||
        !Array.isArray(v.required) ||
        !v.required.every(
          (k) => typeof k === 'string' && own(v.properties as object, k),
        ) ||
        v.additionalProperties !== false
      )
        throw new Error('Invalid record schema')
      s.properties = Object.fromEntries(
        Object.entries(v.properties).map(([k, x]) => [k, read(x, depth + 1)]),
      )
      s.required = v.required as string[]
      s.additionalProperties = false
    }
    if (s.type === 'array') s.items = read(v.items, depth + 1)
    return s
  }
  return read(value, 0)
}
export function nullable(s: Schema): boolean {
  return !!s.anyOf || s.type === 'null'
}
export function concrete(s: Schema): Schema {
  return s.anyOf?.find((x) => x.type !== 'null') ?? s
}
export function fieldKind(s: Schema): string | undefined {
  const c = concrete(s)
  if (s['x-kind'] ?? c['x-kind']) return s['x-kind'] ?? c['x-kind']
  const description = s.description ?? c.description ?? ''
  if (/(?:^|\s)unit id$/.test(description)) return 'unit'
  if (c.pattern === '^[A-E][0-9]{4}$' || /(?:^|\s)hex id$/.test(description))
    return 'hex'
  // Setup destinations are Choice schemas, whose enum can also contain an off-map box.
  if (
    c.enum?.some((id) => /^[A-E][0-9]{4}$/.test(id)) &&
    c.enum.every((id) => /^[A-E][0-9]{4}$/.test(id) || id.startsWith('box_'))
  )
    return 'hex'
  if (c.type === 'array' && c.items?.pattern === '^[A-E][0-9]{4}$')
    return 'path'
  return undefined
}
export function defaultDraft(s: Schema): Value {
  if (nullable(s)) return null
  switch (s.type) {
    case 'string':
      return s.enum?.[0] ?? ''
    case 'integer':
      return s.minimum ?? 0
    case 'boolean':
      return false
    case 'object':
      return Object.fromEntries(
        Object.entries(s.properties ?? {})
          .filter(([k]) => s.required?.includes(k))
          .map(([k, v]) => [k, defaultDraft(v)]),
      )
    case 'array':
      return Array.from({ length: Math.min(s.minItems ?? 0, 1000) }, () =>
        defaultDraft(s.items!),
      )
    default:
      return null
  }
}
export function validateDraft(
  s: Schema,
  v: unknown,
  path = 'action',
): string[] {
  if (s.anyOf) return v === null ? [] : validateDraft(concrete(s), v, path)
  const fail = (message: string) => [`${path}: ${message}`]
  switch (s.type) {
    case 'null':
      return v === null ? [] : fail('expected pass')
    case 'string':
      if (typeof v !== 'string') return fail('expected text')
      if (s.enum && !s.enum.includes(v)) return fail('choose a listed value')
      if (s.pattern && !/^[A-E][0-9]{4}$/.test(v))
        return fail('expected a printed hex id')
      if (
        [...v].length < (s.minLength ?? 0) ||
        [...v].length > (s.maxLength ?? Infinity)
      )
        return fail(
          `text length must be ${s.minLength ?? 0} to ${s.maxLength ?? 'unbounded'}`,
        )
      return []
    case 'integer':
      return typeof v === 'number' &&
        Number.isSafeInteger(v) &&
        v >= (s.minimum ?? -Infinity) &&
        v <= (s.maximum ?? Infinity)
        ? []
        : fail(
            `expected an integer from ${s.minimum ?? '-infinity'} to ${s.maximum ?? 'infinity'}`,
          )
    case 'boolean':
      return typeof v === 'boolean' ? [] : fail('expected true or false')
    case 'object':
      if (!object(v)) return fail('expected an object')
      return [
        ...Object.keys(v)
          .filter((k) => !own(s.properties!, k))
          .map((k) => `${path}: unknown field ${k}`),
        ...Object.entries(s.properties!).flatMap(([k, child]) =>
          !own(v, k)
            ? s.required!.includes(k)
              ? [`${path}.${k}: required`]
              : []
            : validateDraft(child, v[k], `${path}.${k}`),
        ),
      ].slice(0, 50)
    case 'array':
      if (!Array.isArray(v)) return fail('expected an ordered list')
      if (v.length < (s.minItems ?? 0) || v.length > (s.maxItems ?? Infinity))
        return fail(
          `list length must be ${s.minItems ?? 0} to ${s.maxItems ?? 'unbounded'}`,
        )
      return v
        .flatMap((x, i) => validateDraft(s.items!, x, `${path}[${i}]`))
        .slice(0, 50)
    default:
      return fail('unsupported schema')
  }
}
// Preserve valid parts after revision changes. Invalid enum choices become visibly unselected.
export function reconcileDraft(s: Schema, old: Value | undefined): Value {
  if (old === undefined) return defaultDraft(s)
  if (nullable(s) && old === null) return null
  const c = concrete(s)
  if (c.type === 'object' && object(old))
    return Object.fromEntries(
      Object.entries(c.properties!)
        .filter(([k]) => c.required!.includes(k) || own(old, k))
        .map(([k, child]) => [
          k,
          reconcileDraft(child, old[k] as Value | undefined),
        ]),
    )
  if (c.type === 'array' && Array.isArray(old))
    return old
      .slice(0, c.maxItems ?? old.length)
      .map((x) => reconcileDraft(c.items!, x))
  if (!validateDraft(s, old).length) return old
  if (c.enum) return ''
  return defaultDraft(s)
}
export function passOnly(s: Schema): boolean {
  const possible = (c: Schema): boolean =>
    c.anyOf
      ? c.anyOf.some(possible)
      : c.enum
        ? c.enum.length > 0
        : c.type === 'object'
          ? c.required!.every((k) => possible(c.properties![k]))
          : c.type === 'array'
            ? (c.minItems ?? 0) === 0 || possible(c.items!)
            : true
  return s.type === 'null' || (!!s.anyOf && !possible(concrete(s)))
}
