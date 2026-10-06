import { expect, test } from '@playwright/test'
test('streams, inspects, buffers history and replaces perspectives', async ({
  page,
}) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.goto('/')
  await expect(page.getByTestId('playback-status')).toHaveText('● LIVE')
  await expect(page.locator('.board canvas')).toBeVisible()
  await expect(page.locator('.map-error')).toHaveCount(0)
  await expect(page.locator('.map-caption')).toContainText('7,023')
  await expect(page.locator('.tool-result').first()).toContainText(
    'Forward positions returned',
  )
  await page.locator('.unit-row').first().click()
  await expect(page.locator('.unit-detail')).toContainText(
    'Synthetic display values',
  )
  await page
    .getByRole('button', { name: 'Pause playback', exact: true })
    .click()
  const frozen = await page.getByTestId('playback-status').textContent()
  await page.waitForTimeout(2000)
  await expect(page.getByTestId('playback-status')).toHaveText(frozen!)
  await page.getByRole('button', { name: 'Step →', exact: true }).click()
  await expect(page.getByTestId('playback-status')).not.toHaveText(frozen!)
  await page.getByRole('button', { name: 'Return to live' }).click()
  await expect(page.getByTestId('playback-status')).toHaveText('● LIVE')
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:axis')
  await expect(page.getByRole('tab')).toHaveCount(2)
  await expect(page.locator('.formations')).not.toContainText('Demo 11')
  await page.getByRole('tab', { name: 'axis · logistics' }).click()
  await expect(page.locator('.transcript-scroll')).toContainText(
    'Coastal route',
  )
  await page.getByLabel('Transcript filter').selectOption('System 1')
  await expect(page.locator('.entry-system')).toHaveCount(0)
  await page.getByLabel('Perspective', { exact: true }).selectOption('operator')
  await expect(page.getByRole('tab')).toHaveCount(3)
  await page
    .getByRole('button', { name: 'Pause campaign (mock)', exact: true })
    .click()
  await expect(
    page.getByRole('button', { name: 'Resume campaign (mock)' }),
  ).toBeEnabled()
  await page.getByRole('button', { name: 'Resume campaign (mock)' }).click()
  await page.locator('.event-feed button:not([disabled])').first().click()
  await expect(page.locator('.inspector h2')).not.toHaveText('Select a hex')
  await page.getByRole('tab', { name: 'axis · commander' }).click()
  await page.screenshot({ path: '../..//board-real-grid.png', fullPage: true })
  expect(errors).toEqual([])
})
test('navigates the 10k synthetic fixture', async ({ page }) => {
  await page.goto('/?map=synthetic')
  await expect(page.locator('.board canvas')).toBeVisible()
  await expect(page.locator('.map-caption')).toContainText('10,000')
  const canvas = page.locator('.board canvas'),
    bounds = (await canvas.boundingBox())!
  await page.mouse.move(
    bounds.x + bounds.width / 2,
    bounds.y + bounds.height / 2,
  )
  await page.mouse.down()
  await page.mouse.move(
    bounds.x + bounds.width / 2 + 180,
    bounds.y + bounds.height / 2 + 60,
    { steps: 50 },
  )
  await page.mouse.up()
  await page.mouse.wheel(0, -320)
  await expect(page.locator('.map-error')).toHaveCount(0)
  await expect(page.getByTestId('fps')).not.toHaveText('0 FPS · WebGL')
  await page.screenshot({ path: '../..//board-synthetic.png', fullPage: true })
})
