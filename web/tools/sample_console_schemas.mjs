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
  seen = new Map()
// Ignore changing ids/labels in the signature, while preserving the full actual schema in each fixture.
const shape = (s) =>
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
              ? { count: v.length }
              : Array.isArray(v)
                ? v.map(shape)
                : shape(v),
          ]),
      )
    : s
const record = (d) => {
  if (!d.space) return
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
        for (const d of m.view.pending) record(d)
        void api(`${base}/resume`, {}).catch(reject)
      }
      if (m.type === 'event' && m.event.kind === 'decision_opened')
        record(m.event.decision)
      if (m.type === 'resync')
        reject(new Error('Fixture stream lagged; refusing incomplete sample'))
    })
    socket.addEventListener('error', () =>
      reject(new Error('Fixture stream failed')),
    )
    let timer
    const poll = async () => {
      try {
        const status = await api(`${base}?perspective=operator`)
        if (['finished', 'stopped'].includes(status.status.state)) {
          if (status.status.state !== 'finished')
            throw new Error('Scripted fixture campaign stopped')
          resolve()
          return
        }
        timer = setTimeout(poll, 1000)
      } catch (e) {
        reject(e)
      }
    }
    timer = setTimeout(poll, 1000)
    socket.addEventListener('close', () => clearTimeout(timer))
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
        samples: [...seen.values()],
      },
      null,
      2,
    ) + '\n',
  )
  console.log(`Recorded ${seen.size} actual schema variants; no paid calls`)
} finally {
  socket.close()
  await api(`${base}/pause`, {}).catch(() => {})
}
