import { captureSeatLink, createSeatClient, type SeatClient } from './client'
export interface Boot {
  client: SeatClient
  server: string
  token: string
}
// Created once before React mounts. No storage fallback or credential input form.
export function bootConsole(): Boot {
  const link = captureSeatLink(new URL(location.href), (url) =>
    history.replaceState(null, '', url),
  )
  return {
    client: createSeatClient(link),
    server: link.server,
    token: link.token,
  }
}
