import { expect, test } from '@playwright/test'
const token = 'b'.repeat(64)
test('blocks real API and sockets without a capability, then captures a session-scoped credential', async ({
  page,
}) => {
  const paths: string[] = []
  page.on('request', (r) => {
    if (new URL(r.url()).pathname.startsWith('/api/'))
      paths.push(new URL(r.url()).pathname)
  })
  let sockets = 0
  page.on('websocket', (s) => {
    if (s.url().includes('/api/')) sockets++
  })
  await page.route('**/api/session', (route) => {
    expect(route.request().headers().authorization).toBe(`Bearer ${token}`)
    return route.fulfill({
      json: {
        operator: false,
        campaign_id: 'bound',
        perspective: 'seat:axis.commander',
      },
    })
  })
  await page.route('**/api/campaigns/bound?perspective=*', (route) => {
    expect(route.request().headers().authorization).toBe(`Bearer ${token}`)
    return route.fulfill({ json: { status: { state: 'paused' } } })
  })
  const subscribed: string[] = []
  await page.routeWebSocket('**/api/campaigns/bound/stream?cap=*', (socket) => {
    expect(new URL(socket.url()).searchParams.get('cap')).toBe(token)
    socket.onMessage((data) => {
      subscribed.push(JSON.parse(String(data)).perspective)
    })
  })
  await page.goto('/?server=http://127.0.0.1:5173')
  await expect(page.getByRole('status')).toHaveText('Campaign access required')
  expect(paths).toEqual([])
  expect(sockets).toBe(0)
  await page.getByLabel('Campaign capability').fill(token)
  await page.getByRole('button', { name: 'Connect', exact: true }).click()
  await expect.poll(() => subscribed).toEqual(['seat:axis.commander'])
  await expect(page.getByLabel('Perspective', { exact: true })).toHaveValue(
    'seat:axis.commander',
  )
  await expect(
    page
      .getByLabel('Perspective', { exact: true })
      .locator('option[value="operator"]'),
  ).toBeDisabled()
  await expect(
    page.getByRole('button', { name: 'Resume campaign' }),
  ).toBeDisabled()
  await page.reload()
  await expect.poll(() => subscribed.length).toBe(2)
  await page.getByRole('button', { name: 'Forget access' }).click()
  await expect(page.getByRole('status')).toHaveText('Campaign access required')
  await expect(page.locator('.board')).toHaveCount(0)
})
test('removes a supplied fragment and denies a token bound to another campaign before transport', async ({
  page,
}) => {
  let sockets = 0
  page.on('websocket', (s) => {
    if (s.url().includes('/api/')) sockets++
  })
  await page.route('**/api/session', (route) =>
    route.fulfill({
      json: { operator: false, campaign_id: 'own', perspective: 'side:axis' },
    }),
  )
  await page.goto(`/?campaign=other#cap=${token}&tab=map`)
  await expect(page).toHaveURL(/campaign=other#tab=map$/)
  await expect(page.getByRole('status')).toHaveText(
    'This access belongs to a different campaign',
  )
  expect(sockets).toBe(0)
  await expect(page.locator('.board')).toHaveCount(0)
})
test('reports invalid access without rendering private viewer state', async ({
  page,
}) => {
  await page.route('**/api/session', (route) =>
    route.fulfill({ status: 401, body: 'secret internal detail' }),
  )
  await page.goto(`/?campaign=fixture#cap=${token}`)
  await expect(page.getByRole('status')).toContainText(
    'access expired or invalid',
  )
  await expect(page.locator('.board')).toHaveCount(0)
  await expect(page.locator('body')).not.toContainText('secret internal detail')
  await expect(page).toHaveURL(/campaign=fixture$/)
})
test('supports capabilities when sessionStorage is unavailable', async ({
  page,
}) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, 'sessionStorage', {
      get() {
        throw new Error('blocked')
      },
    })
  })
  await page.route('**/api/session', (route) =>
    route.fulfill({
      json: { operator: true, campaign_id: null, perspective: 'operator' },
    }),
  )
  await page.route('**/api/campaigns', (route) => route.fulfill({ json: [] }))
  await page.goto(`/?server=http://127.0.0.1:5173#cap=${token}`)
  await expect(page.getByLabel('Campaign', { exact: true })).toBeVisible()
  await expect(page).toHaveURL(/5173$/)
})

test('discards the loaded viewer when the backend rejects an expired capability', async ({
  page,
}) => {
  await page.route('**/api/session', (route) =>
    route.fulfill({
      json: { operator: true, campaign_id: null, perspective: 'operator' },
    }),
  )
  await page.route('**/api/campaigns', (route) =>
    route.fulfill({ status: 401, body: 'do not render this' }),
  )
  await page.goto(`/?server=http://127.0.0.1:5173#cap=${token}`)
  await expect(page.getByRole('status')).toContainText('expired or invalid')
  await expect(page.locator('.board')).toHaveCount(0)
  await expect(page.getByLabel('Campaign capability')).toBeVisible()
  await expect(page.locator('body')).not.toContainText('do not render this')
  expect(
    await page.evaluate(() =>
      sessionStorage.getItem('cna:cap:http://127.0.0.1:5173'),
    ),
  ).toBeNull()
})
