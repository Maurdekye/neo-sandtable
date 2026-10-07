import { expect, test } from '@playwright/test'
test('renders generated layer stress data and switches explicit coverage lenses', async ({
  page,
}) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.goto('/?layers=fixture&fixture=dense&paused=1')
  await expect(page.locator('.board canvas')).toBeVisible()
  await expect(page.getByTestId('layer-caption')).toContainText(
    'SYNTHETIC LAYER FIXTURE',
  )
  await expect(page.getByTestId('layer-caption')).toContainText('2061')
  await page.getByLabel('Survey coverage').selectOption('side:escarpment')
  await expect(page.getByTestId('layer-caption')).toContainText(
    'unknown escarpment',
  )
  await expect(page.locator('.edge-inspector')).toContainText(
    'escarpment survey',
  )
  await page.locator('.edge-inspector summary').click()
  await expect(page.locator('.edge-inspector')).toContainText('unknown')
  await page.getByLabel('escarpment', { exact: true }).uncheck()
  await page.getByLabel('escarpment', { exact: true }).check()
  await page.getByLabel('Survey coverage').selectOption('terrain')
  await expect(page.getByTestId('layer-caption')).toContainText(
    'unknown terrain',
  )
  await page.getByLabel('Survey coverage').selectOption('off')
  await expect(page.getByTestId('layer-caption')).toContainText('hatch off')
  await page.locator('.rules-coverage summary').click()
  await expect(page.locator('.rules-coverage table')).toContainText(
    'opstage.movement and combat.movement',
  )
  await page.getByLabel('Survey coverage').selectOption('line:road')
  await page.screenshot({
    path: '../../board-layers-stress.png',
    fullPage: true,
  })
  expect(errors).toEqual([])
})
