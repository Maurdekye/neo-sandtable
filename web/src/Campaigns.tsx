import { useEffect, useState } from 'react'
import type { CampaignMeta, Perspective } from './protocol'
import { decodeMessage } from './stream/wire'
async function request(
  server: string,
  path: string,
  options?: RequestInit,
): Promise<unknown> {
  const response = await fetch(new URL(path, server), {
    ...options,
    signal: options?.signal
      ? AbortSignal.any([options.signal, AbortSignal.timeout(10000)])
      : AbortSignal.timeout(10000),
  })
  if (!response.ok)
    throw new Error(`Campaign request failed (${response.status})`)
  return response.json()
}
export function CampaignChooser({ server }: { server: string }) {
  const [campaigns, setCampaigns] = useState<CampaignMeta[]>([]),
    [note, setNote] = useState('Loading campaigns')
  useEffect(() => {
    const abort = new AbortController()
    void request(server, '/api/campaigns', { signal: abort.signal })
      .then((data) => {
        if (!Array.isArray(data)) throw new Error('Invalid campaign list')
        const rows = data.map((campaign) => {
          const hello = decodeMessage(
            JSON.stringify({
              type: 'hello',
              protocol: 1,
              perspective: 'operator',
              campaign,
            }),
          )
          if (hello?.type !== 'hello')
            throw new Error('Invalid campaign metadata')
          return hello.campaign
        })
        if (abort.signal.aborted) return
        setCampaigns(rows)
        setNote(
          rows.length ? 'Choose a campaign' : 'No campaigns are available',
        )
      })
      .catch((error) => {
        if (!abort.signal.aborted) setNote(String(error))
      })
    return () => abort.abort()
  }, [server])
  return (
    <>
      <span className="view-note">{note}</span>
      <select
        aria-label="Campaign"
        value=""
        onChange={(e) => {
          const url = new URL(location.href)
          url.searchParams.set('campaign', e.target.value)
          location.assign(url.href)
        }}
      >
        <option value="">Choose a campaign</option>
        {campaigns.map((c) => (
          <option value={c.id} key={c.id}>
            {c.title}
          </option>
        ))}
      </select>
    </>
  )
}
export function CampaignControl({
  server,
  campaign,
  perspective,
  history,
}: {
  server: string
  campaign: string
  perspective: Perspective
  history: boolean
}) {
  const [state, setState] = useState('loading'),
    [busy, setBusy] = useState(false),
    [error, setError] = useState('')
  const path = `/api/campaigns/${encodeURIComponent(campaign)}`
  useEffect(() => {
    const abort = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    async function refresh() {
      try {
        const data = await request(
          server,
          `${path}?perspective=${encodeURIComponent(perspective)}`,
          { signal: abort.signal },
        )
        if (
          typeof data !== 'object' ||
          data === null ||
          !('status' in data) ||
          typeof data.status !== 'object' ||
          data.status === null ||
          !('state' in data.status) ||
          typeof data.status.state !== 'string'
        )
          throw new Error('Invalid campaign status')
        if (!abort.signal.aborted) {
          setState(data.status.state)
          setError('')
        }
      } catch (error) {
        if (!abort.signal.aborted) setError(String(error))
      }
      if (!abort.signal.aborted) timer = setTimeout(() => void refresh(), 5000)
    }
    void refresh()
    return () => {
      abort.abort()
      clearTimeout(timer)
    }
  }, [server, path, perspective])
  async function toggle() {
    setBusy(true)
    setError('')
    try {
      const data = await request(
        server,
        `${path}/${state === 'paused' ? 'resume' : 'pause'}`,
        { method: 'POST' },
      )
      if (
        typeof data !== 'object' ||
        data === null ||
        !('paused' in data) ||
        typeof data.paused !== 'boolean'
      )
        throw new Error('Invalid campaign control acknowledgement')
      setState(data.paused ? 'paused' : 'running')
    } catch (error) {
      setError(String(error))
    } finally {
      setBusy(false)
    }
  }
  return (
    <>
      <span className="view-note" role="status">
        Campaign {state}
        {error && ` · ${error}`}
      </span>
      <button
        disabled={
          busy ||
          history ||
          perspective !== 'operator' ||
          !['running', 'paused'].includes(state)
        }
        onClick={() => void toggle()}
      >
        {busy
          ? 'Applying control…'
          : state === 'paused'
            ? 'Resume campaign'
            : 'Pause campaign'}
      </button>
    </>
  )
}
