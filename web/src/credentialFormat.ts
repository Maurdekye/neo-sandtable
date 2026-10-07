/** Capability syntax only; importing the transport cannot import storage or spectator access. */
export function validToken(token: string): boolean {
  return /^[a-f0-9]{64}$/.test(token)
}
