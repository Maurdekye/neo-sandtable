import { expect, test } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import type { ViewState } from '../src/protocol'

const server = process.env.CNA_SMOKE_SERVER,
  capability = process.env.CNA_SMOKE_CAPABILITY
const headers = { Authorization: `Bearer ${capability ?? ''}` }
test('watches an actual legal CNA move, remaining units and reconnect highlights', async ({
  page,
  request,
}) => {
  test.skip(
    !server || !capability,
    'Set CNA_SMOKE_SERVER and CNA_SMOKE_CAPABILITY',
  )
  test.setTimeout(90000)
  await page.addInitScript(
    ({ server, capability }) =>
      sessionStorage.setItem(`cna:cap:${new URL(server).origin}`, capability),
    { server: server!, capability: capability! },
  )
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  const created = await request.post(`${server}/api/campaigns`, {
    headers,
    data: {
      kind: 'cna',
      rules_profile: 'cna-2021-dev',
      seed: Array(32).fill(0),
      title: 'CNA movement viewer proof',
      paused: true,
      controller: 'legal_random',
    },
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json(),
    base = `${server}/api/campaigns/${meta.id}`,
    seat = 'axis.front_line'
  expect(
    (
      await request.post(`${base}/seats/${seat}/controller`, {
        headers,
        data: {
          controller: { kind: 'human', label: 'Movement viewer proof' },
          config: {},
        },
      })
    ).ok(),
  ).toBeTruthy()
  await page.goto(`${server}/?campaign=${meta.id}`)
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await page
    .getByRole('button', { name: 'Resume campaign', exact: true })
    .click()
  const movable = page.locator(
    `.pending-decisions article[data-seat="${seat}"] .movable-units`,
  )
  await expect(movable).toBeVisible({ timeout: 20000 })
  const projection = await (await request.get(base, { headers })).json(),
    view: ViewState = projection.snapshot.view
  const decision = view.pending.find(
    (d) => d.seat === seat && d.kind === 'cna.movement.orders',
  )!
  expect(decision).toBeTruthy()
  const schema = decision.space as {
    anyOf: { items?: { properties: { unit: { enum: string[] } } } }[]
  }
  const list = schema.anyOf.find((s) => s.items)!.items!
  const ids = list.properties.unit.enum
  expect(ids.length).toBeGreaterThan(0)
  await expect(movable.locator('button')).toHaveCount(ids.length)
  const lines = readFileSync('../data/map/hexes.csv', 'utf8')
      .trim()
      .split(/\r?\n/),
    keys = lines.shift()!.split(',')
  const hexes = lines.map((line) =>
    Object.fromEntries(line.split(',').map((v, i) => [keys[i], v])),
  )
  let order: { unit: string; path: string[] } | undefined
  for (const id of ids.slice(0, 12)) {
    const origin = hexes.find((h) => h.hex_id === view.units[id].hex)!
    if (!origin) continue
    const adjacent = hexes.filter((h) =>
      [
        [1, 0],
        [0, 1],
        [-1, 1],
        [-1, 0],
        [0, -1],
        [1, -1],
      ].some(
        ([dq, dr]) =>
          Number(origin.q) + dq === Number(h.q) &&
          Number(origin.r) + dr === Number(h.r),
      ),
    )
    for (const hex of adjacent) {
      const candidate = { unit: id, path: [hex.hex_id] }
      const valid = await request.post(
        `${base}/seats/${seat}/decisions/${decision.id}/validate`,
        { headers, data: { action: [candidate] } },
      )
      if (valid.ok()) {
        order = candidate
        break
      }
    }
    if (order) break
  }
  expect(
    order,
    'A disclosed eligible unit must have a validated adjacent move',
  ).toBeTruthy()
  await page
    .getByRole('button', { name: 'land:8.11', exact: true })
    .first()
    .hover()
  await expect(page.getByRole('tooltip')).toContainText('registry paraphrase')
  await expect(page.getByRole('tooltip').locator('strong')).not.toHaveText(
    'No registry entry',
  )
  await page.screenshot({
    path: '../../board-cna-rule-card.png',
    fullPage: true,
  })
  await page.mouse.move(1000, 100)
  const observation = await (
    await request.get(`${base}/seats/${seat}/observe`, { headers })
  ).json()
  const core = observation.pending.find(
    (d: { id: string }) => d.id === decision.id,
  )
  const animation = page.waitForFunction(
    () =>
      Number(
        document
          .querySelector('[data-testid="motion-count"]')
          ?.textContent?.split(' ')[0],
      ) > 0,
    {},
    { timeout: 5000 },
  )
  const submitted = await request.post(
    `${base}/seats/${seat}/decisions/${decision.id}/submit`,
    {
      headers,
      data: {
        decision_id: decision.id,
        seat,
        controller_epoch: observation.controller_epoch,
        decision_revision: core.revision,
        idempotency_key: 'viewer-proof-move',
        action: [order],
        public_explanation: null,
      },
    },
  )
  expect(submitted.ok()).toBeTruthy()
  await animation
  await expect(movable.locator('button')).not.toHaveCount(ids.length)
  async function select() {
    await page.getByLabel('Find formation or unit').fill(order!.unit)
    await page.locator(`.formation-unit[data-unit-id="${order!.unit}"]`).click()
  }
  await select()
  await expect(page.locator('.unit-detail')).toContainText('Moved this segment')
  await expect(
    page.locator(`.unit-row[data-unit-id="${order!.unit}"]`),
  ).toHaveClass(/moved/)
  await page.screenshot({
    path: '../../board-cna-movement.png',
    fullPage: true,
  })
  await page.reload()
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await select()
  await expect(page.locator('.unit-detail')).toContainText('Moved this segment')
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(page.getByTestId('motion-count')).toContainText('0 active')
  await expect(page.locator('.movable-units')).toHaveCount(0)
  await expect(page.locator('.map-error')).toHaveCount(0)
  expect(errors).toEqual([])
  await request.post(`${base}/pause`, { headers })
  writeFileSync(
    '../../movement-browser-verification.json',
    JSON.stringify(
      {
        campaign: meta.id,
        unit: order!.unit,
        from: view.units[order!.unit].hex,
        path: order!.path,
        eligible_before: ids.length,
        actual_animation: true,
        reconnect_highlight: true,
        opponent_private: true,
        errors,
      },
      null,
      2,
    ),
  )
})
