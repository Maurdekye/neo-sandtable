import { expect, test } from '@playwright/test'
test('inspects dense stacks, OA descendants and units outside the map', async ({
  page,
}) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.goto('/?fixture=dense&paused=1')
  await expect(
    page.getByRole('button', { name: 'Resume campaign (mock)' }),
  ).toBeEnabled()
  await expect(page.locator('.board canvas')).toBeVisible()
  await page.getByLabel('Find formation or unit').fill('Synthetic 1.20')
  await page
    .locator('.formation-unit')
    .filter({ hasText: 'Synthetic 1.20' })
    .click()
  await expect(page.locator('.unit-row')).toHaveCount(20)
  await expect(page.locator('.unit-detail')).toContainText('Synthetic 1.20')
  await expect(page.locator('.oa-branch').first()).toContainText(
    'axis-synthetic-army',
  )
  await page.getByLabel('Find unit in axis stack').fill('Synthetic 1.20')
  await expect(page.locator('.unit-row')).toHaveCount(1)
  await page.locator('.unit-row').click()
  await expect(page.locator('.unit-detail')).toContainText('Synthetic 1.1')
  await page.getByLabel('Find formation or unit').fill('Synthetic reserve 1')
  await page
    .locator('.formation-unit')
    .filter({ hasText: 'Synthetic reserve 1' })
    .click()
  await expect(page.locator('.inspector h2')).toHaveText('Awaiting setup')
  await expect(page.locator('.unit-detail')).toContainText(
    'Synthetic reserve 1',
  )
  await page.getByLabel('Find formation or unit').fill('Synthetic reserve 4')
  await page
    .locator('.formation-unit')
    .filter({ hasText: 'Synthetic reserve 4' })
    .click()
  await expect(page.locator('.inspector h2')).toHaveText(
    'Off-map · box_tripoli',
  )
  await expect(page.locator('.unit-detail')).toContainText(
    'Synthetic reserve 4',
  )
  await expect(page.locator('.pending-decisions')).toContainText(
    'Place synthetic reserves',
  )
  await page.getByLabel('Find formation or unit').fill('')
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:axis')
  await expect(page.locator('.formations')).not.toContainText(
    'Synthetic reserve 4',
  )
  await expect(page.locator('.unit-detail')).toHaveCount(0)
  await page.getByLabel('Perspective', { exact: true }).selectOption('operator')
  await page
    .getByRole('button', { name: 'Pause campaign (mock)', exact: true })
    .click()
  await page.getByLabel('Find formation or unit').fill('Synthetic 1.20')
  await page
    .locator('.formation-unit')
    .filter({ hasText: 'Synthetic 1.20' })
    .click()
  await expect(page.getByTestId('fps')).not.toHaveText('0 FPS · WebGL')
  await page.screenshot({
    path: '../../board-dense-formation.png',
    fullPage: true,
  })
  await expect(page.locator('.map-error')).toHaveCount(0)
  expect(errors).toEqual([])
})
