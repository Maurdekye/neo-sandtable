import { expect, test } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import type { ServerMessage, ViewState } from '../src/protocol'
const server = process.env.CNA_SMOKE_SERVER,
  capability = process.env.CNA_SMOKE_CAPABILITY
const headers = { Authorization: `Bearer ${capability ?? ''}` }
test('watches actual baseline Graziani movement and reports authorized locator coverage', async ({
  page,
  request,
}) => {
  test.skip(!server || !capability, 'Set authenticated smoke server')
  test.setTimeout(240000)
  const started = Date.now(),
    errors: string[] = [],
    packets: Extract<ServerMessage, { type: 'event' }>[] = [],
    origins = new Map<string, string | null>()
  page.on('pageerror', (e) => errors.push(e.message))
  const lines = readFileSync('../data/map/hexes.csv', 'utf8')
      .trim()
      .split(/\r?\n/),
    head = lines.shift()!.split(',')
  const terrain = new Map(
    lines.map((line) => {
      const cells = line.split(',')
      return [
        cells[head.indexOf('hex_id')],
        cells[head.indexOf('terrain')],
      ] as const
    }),
  )
  let resolveMove: (m: Extract<ServerMessage, { type: 'event' }>) => void
  const firstMove = new Promise<Extract<ServerMessage, { type: 'event' }>>(
    (resolve) => {
      resolveMove = resolve
    },
  )
  page.on('websocket', (socket) =>
    socket.on('framereceived', (frame) => {
      if (typeof frame.payload !== 'string') return
      const m = JSON.parse(frame.payload) as ServerMessage
      if (m.type === 'snapshot')
        Object.values(m.view.units).forEach((u) => origins.set(u.id, u.hex))
      if (m.type !== 'event') return
      packets.push(m)
      if (
        m.event.kind === 'unit_moved' &&
        m.event.path.length &&
        m.event.path.every(
          (id) => terrain.has(id) && terrain.get(id) !== 'unclassified',
        ) &&
        origins.get(m.event.unit_id) &&
        terrain.get(origins.get(m.event.unit_id)!) !== 'unclassified'
      )
        resolveMove(m)
      if (m.event.kind === 'unit_updated')
        origins.set(m.event.unit.id, m.event.unit.hex)
    }),
  )
  const created = await request.post(`${server}/api/campaigns`, {
    headers,
    data: {
      kind: 'cna',
      rules_profile: 'cna-2021-dev',
      seed: Array(32).fill(0),
      title: 'Graziani coast - scripted movement',
      paused: true,
      controller: 'legal_random',
    },
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json(),
    base = `${server}/api/campaigns/${meta.id}`
  try {
    await page.addInitScript(
      ({ server, capability }) =>
        sessionStorage.setItem(`cna:cap:${new URL(server).origin}`, capability),
      { server: server!, capability: capability! },
    )
    await page.goto(`${server}/?campaign=${meta.id}`)
    await expect(page.getByTestId('playback-status')).toContainText('LIVE')
    await expect(page.locator('.board canvas')).toBeVisible()
    await page.getByLabel('Terrain classification & corridor').check()
    expect(
      (await request.post(`${base}/resume`, { headers })).ok(),
    ).toBeTruthy()
    const timeout = new Promise<never>((_, reject) =>
      setTimeout(
        () => reject(new Error('No actual classified-area move within180s')),
        180000,
      ),
    )
    const message = await Promise.race([firstMove, timeout])
    expect(message.event.kind).toBe('unit_moved')
    if (message.event.kind !== 'unit_moved') throw new Error('Expectedmovement')
    expect((await request.post(`${base}/pause`, { headers })).ok()).toBeTruthy()
    const id = message.event.unit_id,
      destination = message.event.path.at(-1)!
    await page.getByLabel('Event filter').selectOption('unit_moved')
    const row = page
      .locator('.event-row[data-kind="unit_moved"]')
      .filter({ hasText: id })
      .first()
    await expect(row).toBeVisible()
    await expect(row).toContainText('CP spent')
    await row.getByRole('button', { name: /Locate/ }).click()
    await page.getByLabel('Find formation or unit').fill(id)
    await page.locator(`.formation-unit[data-unit-id="${id}"]`).click()
    await expect(page.locator('.unit-detail')).toContainText('CP spent')
    await expect(page.getByTestId('terrain-classification')).toContainText(
      'Classified:',
    )
    await page.screenshot({
      path: '../../board-graziani-coast-movement.png',
      fullPage: true,
    })
    const view: ViewState = (
      await (await request.get(base, { headers })).json()
    ).snapshot.view
    expect(view.units[id]).toBeTruthy()
    const unlocated: Record<string, number> = {},
      totals: Record<string, number> = {}
    packets.forEach((m) => {
      totals[m.event.kind] = (totals[m.event.kind] ?? 0) + 1
      if (!m.hex) unlocated[m.event.kind] = (unlocated[m.event.kind] ?? 0) + 1
    })
    writeFileSync(
      '../../coast-browser-verification.json',
      JSON.stringify(
        {
          commit: process.env.CNA_SMOKE_COMMIT,
          elapsed_ms: Date.now() - started,
          controller: 'legal_random',
          unit: id,
          path: message.event.path,
          cp_spent: message.event.cp_spent,
          destination,
          classified_path: true,
          total_events: packets.length,
          event_kinds: totals,
          missing_envelope_hex: unlocated,
          combat_seen: packets.some((m) => m.event.kind === 'combat_resolved'),
          errors,
        },
        null,
        2,
      ),
    )
    await page
      .getByLabel('Perspective', { exact: true })
      .selectOption('side:commonwealth')
    await expect(
      page.locator(`.formation-unit[data-unit-id="${id}"]`),
    ).toHaveCount(0)
    await expect(page.getByTestId('motion-count')).toContainText('0 active')
    expect(errors).toEqual([])
  } finally {
    await request.post(`${base}/pause`, { headers })
  }
})
