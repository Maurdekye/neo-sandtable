/* global process, fetch, URL, WebSocket, setTimeout, clearTimeout, console, AbortSignal */
// Authoring tool ONLY. Not imported by the console: its operator credential belongs to the test harness.
import { writeFileSync } from 'node:fs'
const server = process.env.CNA_SMOKE_SERVER,
  token = process.env.CNA_SMOKE_CAPABILITY
if (!server || !token)
  throw new Error(
    'Fresh local scripted fixture server and operator capability required',
  )
const headers = { Authorization: `Bearer ${token}` },
  output = process.argv[2] ?? 'src/console/fixtures/graziani-spaces.json'
const api = async (path, body) => {
  const response = await fetch(new URL(path, server), {
    method: body === undefined ? 'GET' : 'POST',
    headers: { ...headers, 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(15000),
  })
  if (!response.ok)
    throw new Error(`Fixture authoring request failed (${response.status})`)
  return response.json()
}
const campaign = await api('/api/campaigns', {
  kind: 'cna',
  rules_profile: 'cna-2021-dev',
  seed: Array(32).fill(0),
  title: 'Console schema fixture authoring',
  paused: true,
  controller: 'legal_random',
})
const base = `/api/campaigns/${campaign.id}`,
  seen = new Map(),
  variants = new Set()
let lastSeq = null,
  closed = false,
  recoveryTarget = null,
  recoveries = 0,
  recovering = false,
  resumeAfterReplay = false,
  timer
// Ignore changing ids/labels in the signature, while preserving the full actual schema in each fixture.
// Keep empty enums and zero bounds distinct, but do not duplicate a form for each stock total.
// The fixture retains the original full schema of one actual example for every kind/shape.
const shape = (s, structural = true) =>
  s && typeof s === 'object' && !Array.isArray(s)
    ? Object.fromEntries(
        Object.entries(s)
          .filter(
            ([k]) =>
              !['description', 'x-context', 'x-from', 'x-options'].includes(k),
          )
          .map(([k, v]) => [
            k,
            k === 'enum'
              ? {
                  count: structural
                    ? v.length
                      ? 'nonempty'
                      : 'empty'
                    : v.length,
                }
              : structural &&
                  [
                    'minimum',
                    'maximum',
                    'minLength',
                    'maxLength',
                    'minItems',
                    'maxItems',
                  ].includes(k)
                ? v === 0
                  ? 'zero'
                  : v > 0
                    ? 'positive'
                    : 'negative'
                : Array.isArray(v)
                  ? v.map((x) => shape(x, structural))
                  : shape(v, structural),
          ]),
      )
    : s
const record = (d) => {
  if (!d.space) return
  variants.add(d.kind + JSON.stringify(shape(d.space, false)))
  const key = d.kind + JSON.stringify(shape(d.space))
  if (!seen.has(key))
    seen.set(key, { kind: d.kind, rules: d.rules, space: d.space })
}
const url = new URL(`${base}/stream`, server)
url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
url.searchParams.set('cap', token)
const socket = new WebSocket(url),
  done = new Promise((resolve, reject) => {
    socket.addEventListener('open', () =>
      socket.send(
        JSON.stringify({
          type: 'subscribe',
          perspective: 'operator',
          from_seq: null,
        }),
      ),
    )
    socket.addEventListener('message', (event) => {
      const m = JSON.parse(event.data)
      if (m.type === 'snapshot') {
        lastSeq = m.seq
        for (const d of m.view.pending) record(d)
        void api(`${base}/resume`, {}).catch(reject)
      }
      if (m.type === 'event') {
        if (lastSeq !== null && m.seq !== lastSeq + 1) {
          reject(new Error('Fixture event sequence gap'))
          return
        }
        lastSeq = m.seq
        if (recovering && lastSeq === recoveryTarget) {
          recovering = false
          if (resumeAfterReplay && !closed)
            void api(`${base}/resume`, {}).catch(reject)
        }
      }
      if (m.type === 'event' && m.event.kind === 'decision_opened')
        record(m.event.decision)
      if (m.type === 'resync') {
        if (lastSeq === null || recovering) {
          reject(new Error('Fixture replay could not restore continuity'))
          return
        }
        recovering = true
        recoveries++
        // Pause the authoring baseline, then recover every missing event from durable replay.
        // Never replace a gap with a newer snapshot and call it complete coverage.
        void (async () => {
          await api(`${base}/pause`, {})
          const status = await api(`${base}?perspective=operator`)
          recoveryTarget = status.snapshot.seq
          resumeAfterReplay = status.status.state !== 'finished'
          socket.send(
            JSON.stringify({
              type: 'subscribe',
              perspective: 'operator',
              from_seq: lastSeq,
            }),
          )
          if (lastSeq === recoveryTarget) {
            recovering = false
            if (resumeAfterReplay && !closed) await api(`${base}/resume`, {})
          }
        })().catch(reject)
      }
    })
    socket.addEventListener('error', () =>
      reject(new Error('Fixture stream failed')),
    )
    const poll = async () => {
      if (closed) return
      try {
        const status = await api(`${base}?perspective=operator`)
        if (['finished', 'stopped'].includes(status.status.state)) {
          if (status.status.state !== 'finished')
            throw new Error('Scripted fixture campaign stopped')
          if (lastSeq !== null && lastSeq >= status.snapshot.seq) {
            resolve()
            return
          }
        }
        if (!closed) timer = setTimeout(poll, 5000)
      } catch (e) {
        reject(e)
      }
    }
    timer = setTimeout(poll, 5000)
    socket.addEventListener('close', () => {
      closed = true
      clearTimeout(timer)
    })
  })
try {
  await Promise.race([
    done,
    new Promise((_, reject) =>
      setTimeout(
        () =>
          reject(
            new Error('Schema fixture run exceeded bounded 15-minute guard'),
          ),
        900000,
      ).unref(),
    ),
  ])
  writeFileSync(
    output,
    JSON.stringify(
      {
        source_commit: process.env.CNA_SMOKE_COMMIT ?? 'unreported',
        rules_profile: 'cna-2021-dev',
        controller: 'legal_random',
        paid_calls: 0,
        completed: true,
        observed_variants: variants.size,
        sampling:
          'one actual example per kind and structural shape; empty enums and zero bounds distinct',
        replay_recoveries: recoveries,
        final_seq: lastSeq,
        samples: [...seen.values()],
      },
      null,
      2,
    ) + '\n',
  )
  console.log(`Recorded ${seen.size} actual schema variants; no paid calls`)
} finally {
  closed = true
  clearTimeout(timer)
  socket.close()
  await api(`${base}/pause`, {}).catch(() => {})
}
