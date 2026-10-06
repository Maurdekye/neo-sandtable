import { useEffect, useState, type ReactNode } from 'react'
import { actions } from './stream/store'
import {
  apiRequest,
  captureCredential,
  credentialKey,
  decodeSession,
  serverOrigin,
  validToken,
  type Access,
} from './access'
export function AccessGate({
  server,
  campaign,
  children,
}: {
  server: string
  campaign: string | null
  children: (access: Access) => ReactNode
}) {
  const [credential, setCredential] = useState(() => {
    // Remove the fragment even if storage is unavailable or the backend is invalid.
    const url = new URL(location.href)
    let storage: Storage
    try {
      storage = sessionStorage
    } catch {
      storage = {
        getItem: () => null,
        setItem: () => {},
        removeItem: () => {},
      } as unknown as Storage
    }
    try {
      return {
        token: captureCredential(server, url, storage, (href) =>
          history.replaceState(null, '', href),
        ),
        error: '',
      }
    } catch {
      const fragment = new URLSearchParams(url.hash.slice(1))
      fragment.delete('cap')
      url.hash = fragment.toString()
      history.replaceState(null, '', url.href)
      return { token: '', error: 'Use a local HTTP or HTTPS campaign server' }
    }
  })
  const [access, setAccess] = useState<Access | null>(null)
  const [note, setNote] = useState(
    credential.error ||
      (credential.token
        ? 'Checking campaign access'
        : 'Campaign access required'),
  )
  const [input, setInput] = useState('')
  useEffect(() => {
    if (!credential.token || credential.error) return
    const abort = new AbortController()
    void apiRequest({ server, token: credential.token }, '/api/session', {
      signal: abort.signal,
    })
      .then((data) => {
        const session = decodeSession(data)
        if (
          session.campaign_id !== null &&
          campaign !== null &&
          campaign !== session.campaign_id
        )
          throw new Error('This access belongs to a different campaign')
        if (!abort.signal.aborted)
          setAccess({
            server: serverOrigin(server),
            token: credential.token,
            session,
            onUnauthorized: () => {
              if (abort.signal.aborted) return
              actions.clear()
              setAccess(null)
              setNote(
                'Campaign access expired or invalid; open a fresh access link',
              )
              try {
                sessionStorage.removeItem(credentialKey(server))
              } catch {
                /* Storage may be unavailable. */
              }
            },
          })
      })
      .catch((error: unknown) => {
        if (!abort.signal.aborted)
          setNote(
            error instanceof Error
              ? error.message
              : 'Cannot verify campaign access',
          )
      })
    return () => abort.abort()
  }, [server, campaign, credential])
  if (access) return children(access)
  return (
    <main className="access-screen">
      <form
        className="access-form"
        onSubmit={(event) => {
          event.preventDefault()
          if (!validToken(input)) {
            setNote(
              'Enter the 64-character capability from your server access link',
            )
            return
          }
          try {
            sessionStorage.removeItem(credentialKey(server))
            sessionStorage.setItem(credentialKey(server), input)
          } catch {
            /* Memory-only access. */
          }
          setInput('')
          setNote('Checking campaign access')
          setCredential({ token: input, error: '' })
        }}
      >
        <span className="eyebrow">NEO SANDTABLE</span>
        <h1>Open the live board</h1>
        <p role="status">{note}</p>
        <p>
          Open the board access link printed by your campaign server, or enter
          its capability here.
        </p>
        <label>
          Campaign capability
          <input
            type="password"
            autoComplete="off"
            value={input}
            onChange={(event) => setInput(event.target.value)}
          />
        </label>
        <button type="submit" disabled={Boolean(credential.error)}>
          Connect
        </button>
      </form>
    </main>
  )
}
