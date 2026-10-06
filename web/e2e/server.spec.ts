import { expect, test } from '@playwright/test'
const server = process.env.CNA_SMOKE_SERVER
const capability = process.env.CNA_SMOKE_CAPABILITY
const headers = { Authorization: `Bearer ${capability ?? ''}` }
test.beforeEach(async ({ page }) => {
  if (server && capability)
    await page.addInitScript(
      ({ server, capability }) => {
        sessionStorage.setItem(`cna:cap:${new URL(server).origin}`, capability)
      },
      { server, capability },
    )
})
test('watches a real sandbox server and uses operator HTTP controls', async ({
  page,
  request,
}) => {
  test.skip(
    !server || !capability,
    'Set CNA_SMOKE_SERVER to a running own-clone sandbox server',
  )
  const created = await request.post(`${server}/api/campaigns`, {
    headers,
    data: {
      rules_profile: 'sandbox-v1',
      seed: Array(32).fill(12),
      title: 'Board integration fixture',
      paused: true,
      controller: 'aggressive',
    },
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json()
  const subscriptions: { from_seq: number | null; perspective: string }[] = []
  page.on('websocket', (socket) =>
    socket.on('framesent', (frame) => {
      try {
        const message = JSON.parse(String(frame.payload))
        if (message.type === 'subscribe') subscriptions.push(message)
      } catch {
        /* Vite HMR is unrelated. */
      }
    }),
  )
  await page.addInitScript(() => {
    const captured: WebSocket[] = []
    Object.assign(window, { boardTestSockets: captured })
    const NativeSocket = window.WebSocket
    window.WebSocket = class extends NativeSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols)
        if (String(url).includes('/api/campaigns/')) captured.push(this)
      }
    }
  })
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.goto(`/?server=${encodeURIComponent(server!)}`)
  await expect(
    page
      .getByLabel('Campaign', { exact: true })
      .locator(`option[value="${meta.id}"]`),
  ).toHaveText('Board integration fixture')
  await page.getByLabel('Campaign', { exact: true }).selectOption(meta.id)
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(page.locator('.formations .formation button')).toHaveCount(12)
  await expect(page.locator('.transcript-scroll')).not.toContainText('Mock CLI')
  await expect(
    page.getByRole('button', { name: 'Resume campaign', exact: true }),
  ).toBeEnabled()
  await expect(page.getByTestId('fps')).toHaveText(/^[1-9]\d* FPS/)
  await page.screenshot({
    path: '../../board-live-sandbox.png',
    fullPage: true,
  })
  await page
    .getByRole('button', { name: 'Resume campaign', exact: true })
    .click()
  await expect(page.locator('.event-feed button').first()).toBeVisible()
  await expect(page.locator('.entry-decision_submitted').first()).toBeVisible()
  await page.screenshot({
    path: '../../board-live-transcripts.png',
    fullPage: true,
  })
  await expect(page.locator('.viewbar [role="status"]')).toContainText(
    'Campaign finished',
    { timeout: 10000 },
  )
  const before = await page.locator('.entry-decision_submitted').count()
  await page.evaluate(() => {
    ;(window as unknown as { boardTestSockets: WebSocket[] }).boardTestSockets
      .at(-1)!
      .close()
  })
  await expect(page.getByTestId('playback-status')).toHaveText('CONNECTING')
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(page.locator('.entry-decision_submitted')).toHaveCount(before)
  expect(subscriptions.at(-1)?.from_seq).toBeGreaterThan(0)
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(
    page.locator('.formations .formation').first().getByRole('button'),
  ).toHaveCount(0)
  await expect(page.getByRole('tab')).toHaveCount(10)
  await expect(page.locator('.entry-decision_submitted')).toHaveCount(0)
  await page.getByRole('tab', { name: 'commonwealth · commander' }).click()
  await expect(page.locator('.entry-decision_submitted').first()).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Pause campaign', exact: true }),
  ).toBeDisabled()
  await page.screenshot({ path: '../../board-live-side.png', fullPage: true })
  await request.post(`${server}/api/campaigns/${meta.id}/pause`, { headers })
  expect(errors).toEqual([])
})

test('HTTP operator pause and resume stay separate from playback', async ({
  page,
  request,
}) => {
  test.skip(
    !server || !capability,
    'Set CNA_SMOKE_SERVER to a running own-clone sandbox server',
  )
  const created = await request.post(`${server}/api/campaigns`, {
    headers,
    data: {
      rules_profile: 'sandbox-v1',
      seed: Array(32).fill(4),
      paused: true,
      controller: 'human',
    },
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json()
  await page.goto(`/?campaign=${meta.id}&server=${encodeURIComponent(server!)}`)
  await page
    .getByRole('button', { name: 'Resume campaign', exact: true })
    .click()
  await expect(
    page.getByRole('button', { name: 'Pause campaign', exact: true }),
  ).toBeEnabled()
  await page
    .getByRole('button', { name: 'Pause campaign', exact: true })
    .click()
  await expect(
    page.getByRole('button', { name: 'Resume campaign', exact: true }),
  ).toBeEnabled()
  await page
    .getByRole('button', { name: 'Pause playback', exact: true })
    .click()
  await expect(
    page.getByRole('button', { name: 'Resume campaign', exact: true }),
  ).toBeDisabled()
})
