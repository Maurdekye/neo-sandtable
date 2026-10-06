import { expect, test } from '@playwright/test'
import { writeFileSync } from 'node:fs'
import type { ViewState, ServerMessage } from '../src/protocol'
const server = process.env.CNA_SMOKE_SERVER
const payload = {
  kind: 'cna',
  rules_profile: 'cna-2021-dev',
  seed: Array(32).fill(0),
  title: "Graziani's Offensive",
  paused: true,
  controller: 'legal_random',
}
test('watches actual Graziani positions and private scripted decisions in production', async ({
  page,
  request,
}) => {
  test.skip(
    !server,
    'Set CNA_SMOKE_SERVER to a published CNA server with a fresh database directory',
  )
  const created = await request.post(`${server}/api/campaigns`, {
    data: payload,
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json()
  const inspected = await request.get(`${server}/api/campaigns/${meta.id}`)
  expect(inspected.ok()).toBeTruthy()
  const projection = await inspected.json()
  const view: ViewState = projection.snapshot.view
  const units = Object.values(view.units)
  expect(units.length).toBeGreaterThan(100)
  const dense = [...view.stacks].sort(
    (a, b) => b.unit_ids.length - a.unit_ids.length,
  )[0]
  expect(dense.unit_ids.length).toBeGreaterThanOrEqual(10)
  const target = view.units[dense.unit_ids.at(-1)!]
  const awaiting = units.find(
    (u) =>
      u.hex === null &&
      (u.detail?.location as { at?: string })?.at === 'awaiting_setup',
  )!
  const offmap = units.find(
    (u) =>
      u.hex === null &&
      (u.detail?.location as { at?: string })?.at === 'off_map',
  )!
  expect(awaiting).toBeTruthy()
  expect(offmap).toBeTruthy()
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  const incoming: ServerMessage[] = []
  page.on('websocket', (socket) =>
    socket.on('framereceived', (frame) => {
      try {
        const m = JSON.parse(String(frame.payload))
        if (m.type) incoming.push(m)
      } catch {
        /* Vite traffic isn't the campaign stream. */
      }
    }),
  )
  // Actual production bundle served by Rust, rather than Vite's development module graph.
  await page.goto(`${server}/?campaign=${meta.id}`)
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(page.locator('.campaign-title')).toContainText(
    "Graziani's Offensive",
  )
  await expect(page.locator('.map-caption')).toContainText('7,023')
  await expect(page.getByTestId('fps')).not.toContainText('0 FPS')
  async function select(id: string) {
    await page.getByLabel('Find formation or unit').fill(id)
    await page.locator(`.formation-unit[data-unit-id="${id}"]`).click()
    await expect(page.locator('.unit-detail')).toContainText(id)
  }
  await select(target.id)
  await expect(page.locator('.unit-row')).toHaveCount(dense.unit_ids.length)
  await page.getByLabel(`Find unit in ${dense.side} stack`).fill(target.id)
  await expect(page.locator('.unit-row')).toHaveCount(1)
  await page.locator(`.unit-row[data-unit-id="${target.id}"]`).click()
  await page.screenshot({
    path: '../../board-graziani-stack.png',
    fullPage: true,
  })
  const related = units.find(u => u.parent !== null)!
  expect(related).toBeTruthy()
  await select(related.id)
  await expect(page.locator('.oa-branch').first()).toBeVisible()
  await select(awaiting.id)
  await expect(page.locator('.inspector h2')).toContainText('Awaiting setup')
  await expect(page.locator('.unit-detail')).toContainText(
    String((awaiting.detail?.location as { group: string }).group),
  )
  await select(offmap.id)
  await expect(page.locator('.inspector h2')).toContainText('Off-map')
  await expect(page.locator('.unit-detail')).toContainText(
    String((offmap.detail?.location as { id: string }).id),
  )
  const dump = view.markers.find((m) => m.kind === 'supply_dump' && m.label)!
  expect(dump).toBeTruthy()
  await page
    .locator('.formations')
    .getByRole('button', { name: dump.label!, exact: false })
    .click()
  await expect(page.locator('.inspector')).toContainText(dump.label!)
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:axis')
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(
    page.locator('.formations .formation').nth(1).getByRole('button'),
  ).toHaveCount(0)
  const side = await (
    await request.get(
      `${server}/api/campaigns/${meta.id}?perspective=side:axis`,
    )
  ).json()
  expect(
    Object.values(side.snapshot.view.units).every(
      (u: unknown) => (u as { side: string }).side === 'axis',
    ),
  ).toBeTruthy()
  expect(
    side.snapshot.view.markers
      .filter((m: { side: string }) => m.side === 'commonwealth')
      .every((m: { label: string | null }) => m.label === null),
  ).toBeTruthy()
  await page.getByLabel('Perspective', { exact: true }).selectOption('operator')
  await page
    .getByRole('button', { name: 'Resume campaign', exact: true })
    .click()
  await expect(page.locator('.entry-decision_submitted').first()).toBeVisible({
    timeout: 20000,
  })
  await expect(page.locator('.viewbar [role="status"]')).toContainText(
    'Campaign finished',
    { timeout: 20000 },
  )
  const decisions = incoming.filter(
    (m) => m.type === 'transcript' && m.entry.kind === 'decision_submitted',
  )
  expect(decisions).toHaveLength(18)
  await page.screenshot({
    path: '../../board-graziani-transcripts.png',
    fullPage: true,
  })
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(page.locator('.entry-decision_submitted')).toHaveCount(0)
  await page.getByRole('tab', { name: 'commonwealth · commander' }).click()
  await expect(page.locator('.entry-decision_submitted')).toHaveCount(9)
  await expect(page.locator('.map-error')).toHaveCount(0)
  expect(errors).toEqual([])
  writeFileSync(
    '../../graziani-browser-verification.json',
    JSON.stringify(
      {
        adapter: '194c2fd',
        campaign: meta.id,
        units: units.length,
        mapped_units: units.filter((u) => u.hex).length,
        stacks: view.stacks.length,
        largest_stack: dense.unit_ids.length,
        awaiting: units.filter(
          (u) =>
            (u.detail?.location as { at?: string })?.at === 'awaiting_setup',
        ).length,
        offmap: units.filter(
          (u) => (u.detail?.location as { at?: string })?.at === 'off_map',
        ).length,
        dump_markers: view.markers.length,
        decisions: decisions.length,
        errors,
      },
      null,
      2,
    ),
  )
})
test('shows actual CNA decision citations while a human seat holds the window open', async ({
  page,
  request,
}) => {
  test.skip(!server, 'Set CNA_SMOKE_SERVER to a running CNA server')
  const created = await request.post(`${server}/api/campaigns`, {
    data: {
      ...payload,
      controller: 'human',
      title: 'Graziani decision inspection',
    },
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json()
  await page.goto(`/?campaign=${meta.id}&server=${encodeURIComponent(server!)}`)
  await page
    .getByRole('button', { name: 'Resume campaign', exact: true })
    .click()
  await expect(page.getByLabel('Rule citations').first()).toContainText(
    'land:7.11',
  )
  await expect(page.locator('.pending-decisions')).toContainText('initiative')
  const visible = await page.locator('.pending-decisions').boundingBox()
  expect(visible!.y).toBeLessThan(250)
  await expect(page.getByTestId('fps')).not.toContainText('0 FPS')
  await page.screenshot({
    path: '../../board-graziani-decisions.png',
    fullPage: true,
  })
  await request.post(`${server}/api/campaigns/${meta.id}/pause`)
})
