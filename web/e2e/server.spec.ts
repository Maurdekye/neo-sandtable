import { expect, test } from '@playwright/test'
const server = process.env.CNA_SMOKE_SERVER
test('watches a real sandbox server and uses operator HTTP controls', async ({
  page,
  request,
}) => {
  test.skip(
    !server,
    'Set CNA_SMOKE_SERVER to a running own-clone sandbox server',
  )
  const created = await request.post(`${server}/api/campaigns`, {
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
  await expect(page.getByTestId('fps')).not.toContainText('0 FPS')
  await page.screenshot({
    path: '../../board-live-sandbox.png',
    fullPage: true,
  })
  await page
    .getByRole('button', { name: 'Resume campaign', exact: true })
    .click()
  await expect(page.locator('.event-feed button').first()).toBeVisible()
  // Current scripted baseline does not yet publish transcript entries.
  await expect(page.locator('.transcript-scroll')).toContainText('No entries')
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(
    page.locator('.formations .formation').first().getByRole('button'),
  ).toHaveCount(0)
  await expect(page.getByRole('tab')).toHaveCount(10)
  await expect(
    page.getByRole('button', { name: 'Pause campaign', exact: true }),
  ).toBeDisabled()
  await page.screenshot({ path: '../../board-live-side.png', fullPage: true })
  await request.post(`${server}/api/campaigns/${meta.id}/pause`)
  expect(errors).toEqual([])
})

test('HTTP operator pause and resume stay separate from playback', async ({
  page,
  request,
}) => {
  test.skip(
    !server,
    'Set CNA_SMOKE_SERVER to a running own-clone sandbox server',
  )
  const created = await request.post(`${server}/api/campaigns`, {
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
