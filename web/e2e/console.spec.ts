import { expect, test } from '@playwright/test'
import type { Clock, ServerMessage } from '../src/protocol'
const clock: Clock = {
  game_turn: 1,
  op_stage: 1,
  date: '1940-09-15',
  stage: 'setup',
  phase: 'placement',
  segment: null,
  step: null,
  phasing: 'axis',
}
const token = 'a'.repeat(64)
test('seat console renders every form shape, retries revisions and stops at handover without storage or operator calls', async ({
  page,
}) => {
  const errors: string[] = [],
    routes: string[] = [],
    submits: unknown[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.addInitScript(() => {
    Object.defineProperty(Storage.prototype, 'setItem', {
      value: () => {
        throw new Error('Console wrote storage')
      },
    })
    Object.defineProperty(Storage.prototype, 'getItem', {
      value: () => {
        throw new Error('Console read storage')
      },
    })
  })
  let epoch = 4,
    revision = 1,
    stale = true
  let hold = false,
    enteredHeldValidation = false,
    releaseValidation = () => {}
  const heldValidation = new Promise<void>((resolve) => {
    releaseValidation = resolve
  })
  const schema = () => ({
    type: 'object',
    properties: {
      unit: {
        type: 'string',
        enum: revision === 1 ? ['own.a', 'own.b'] : ['own.b'],
        'x-kind': 'unit',
      },
      hex: { type: 'string', enum: ['C4120', 'C4220'], 'x-kind': 'hex' },
      count: { type: 'integer', minimum: 0, maximum: 3 },
      flag: { type: 'boolean' },
      optional: {
        anyOf: [
          { type: 'string', minLength: 2, maxLength: 4 },
          { type: 'null' },
        ],
      },
      orders: {
        type: 'array',
        items: {
          type: 'object',
          properties: { code: { type: 'string', minLength: 1, maxLength: 3 } },
          required: ['code'],
          additionalProperties: false,
        },
        minItems: 0,
        maxItems: 3,
      },
      path: {
        type: 'array',
        items: { type: 'string', pattern: '^[A-E][0-9]{4}$' },
        maxItems: 2,
        'x-kind': 'path',
        'x-from': 'C4120',
      },
      choice: {
        type: 'string',
        enum: ['go', 'done'],
        'x-options': [
          { id: 'go', label: 'Advance', detail: 'Move cautiously' },
          { id: 'done', label: 'Done' },
        ],
      },
    },
    required: ['unit', 'hex', 'count', 'flag', 'orders', 'path', 'choice'],
    additionalProperties: false,
    'x-context': { unit: 'own.a' },
  })
  const request = () => ({
    id: 'axis.commander-1',
    seat: 'axis.commander',
    kind: 'fixture',
    summary: 'Synthetic human order',
    revision,
    rules: ['land:8.1'],
    space: { schema: { type: 'record', fields: [] } },
  })
  await page.route('**/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname
    routes.push(path)
    expect(route.request().headers().authorization).toBe(`Bearer ${token}`)
    if (path === '/api/session')
      return route.fulfill({
        json: {
          operator: false,
          perspective: 'seat:axis.commander',
          campaign_id: 'console-fixture',
        },
      })
    expect(
      path.startsWith('/api/campaigns/console-fixture/seats/axis.commander/'),
    ).toBe(true)
    if (path.endsWith('/observe'))
      return route.fulfill({
        json: {
          controller_epoch: epoch,
          controller: {
            kind: epoch === 4 ? 'human' : 'scripted',
            label: 'Human',
          },
          paused: false,
          failure: null,
          pending: [request()],
        },
      })
    if (path.endsWith('/actions'))
      return route.fulfill({
        json: { request: request(), action_schema: schema() },
      })
    if (path.endsWith('/validate')) {
      const body = route.request().postDataJSON()
      expect(body.controller_epoch).toBe(4)
      if (stale) {
        stale = false
        revision = 2
        return route.fulfill({
          status: 409,
          json: { error: 'decision revision superseded' },
        })
      }
      if (hold) {
        enteredHeldValidation = true
        await heldValidation
      }
      expect(body.decision_revision).toBe(2)
      return route.fulfill({ json: { valid: true } })
    }
    if (path.endsWith('/submit')) {
      submits.push(route.request().postDataJSON())
      return route.fulfill({ json: { accepted: true } })
    }
    if (path.includes('/inspect/'))
      return route.fulfill({ json: { cp: 3, fuel: 1 } })
    throw new Error(`Unexpected console endpoint ${path}`)
  })
  await page.routeWebSocket(
    '**/api/campaigns/console-fixture/stream?cap=*',
    (socket) => {
      socket.onMessage((raw) => {
        expect(JSON.parse(String(raw))).toEqual({
          type: 'subscribe',
          perspective: 'seat:axis.commander',
          from_seq: null,
        })
        const send = (m: ServerMessage) => socket.send(JSON.stringify(m))
        send({
          type: 'hello',
          protocol: 1,
          perspective: 'seat:axis.commander',
          campaign: {
            id: 'console-fixture',
            title: 'Synthetic console',
            scenario_id: 'fixture',
            rules_profile: 'fixture',
            seats: [
              {
                id: 'axis.commander',
                side: 'axis',
                role: 'commander',
                controller: { kind: 'human', label: 'Human' },
                status: 'deciding',
              },
            ],
          },
        })
        send({
          type: 'snapshot',
          seq: 0,
          view: {
            clock,
            units: {
              'own.a': {
                id: 'own.a',
                name: 'Own A',
                side: 'axis',
                kind: 'infantry',
                size: 'battalion',
                nationality: 'italian',
                hex: 'C4120',
                parent: null,
                detail: { cpa: 3 },
              },
            },
            stacks: [
              {
                hex: 'C4120',
                side: 'axis',
                unit_ids: ['own.a'],
                visible_count: 1,
              },
            ],
            markers: [],
            pending: [],
          },
        })
      })
    },
  )
  await page.goto(
    `/console.html?campaign=console-fixture&seat=axis.commander#cap=${token}`,
  )
  await expect(
    page.getByRole('button', { name: 'Validate and submit', exact: true }),
  ).toBeEnabled()
  expect(page.url()).not.toContain(token)
  await page
    .getByRole('button', { name: 'Pick hex on map', exact: true })
    .click()
  await expect(page.getByTestId('placement-highlight')).toContainText(
    '2 enumerated target hexes',
  )
  await page.getByRole('button', { name: 'Stop picking', exact: true }).click()
  await page.getByLabel('count', { exact: true }).fill('2')
  await page.getByLabel('optional', { exact: false }).count() // nullable omit has a visible label
  await page.getByRole('checkbox', { name: 'Omit', exact: true }).uncheck()
  await page.getByLabel('optional', { exact: true }).fill('ok')
  await page.getByRole('button', { name: 'Add orders', exact: true }).click()
  await page.getByLabel('code', { exact: true }).fill('A')
  await page.getByRole('button', { name: 'Add orders', exact: true }).click()
  await page.getByLabel('code', { exact: true }).nth(1).fill('B')
  await page.getByRole('button', { name: 'Up', exact: true }).nth(1).click()
  await page
    .getByRole('button', { name: 'Remove', exact: true })
    .first()
    .click()
  await page
    .getByRole('button', { name: 'Append hex on map', exact: true })
    .click()
  await expect(
    page.getByText('Candidate map picks; legality requires engine preflight'),
  ).toBeVisible()
  await page.getByRole('button', { name: 'Stop picking', exact: true }).click()
  await page.getByRole('button', { name: 'Done', exact: true }).click()
  await page
    .getByLabel('Seat commentary')
    .fill('<img src=x onerror=alert(1)> plain')
  await page
    .getByRole('button', { name: 'Validate and submit', exact: true })
    .click()
  await expect(page.getByLabel('unit', { exact: true })).toHaveValue('')
  await expect(page.getByLabel('count', { exact: true })).toHaveValue('2')
  await expect(page.getByLabel('optional', { exact: true })).toHaveValue('ok')
  expect(submits).toHaveLength(0)
  await page.getByLabel('unit', { exact: true }).selectOption('own.b')
  await page
    .getByRole('button', { name: 'Validate and submit', exact: true })
    .click()
  await expect(
    page.getByText('axis.commander-1: answer accepted', { exact: true }),
  ).toBeVisible()
  expect(submits).toHaveLength(1)
  expect(submits[0]).toMatchObject({
    controller_epoch: 4,
    decision_revision: 2,
    action: {
      unit: 'own.b',
      count: 2,
      orders: [{ code: 'A' }],
      choice: 'done',
    },
    public_explanation: '<img src=x onerror=alert(1)> plain',
  })
  expect(await page.locator('img[src="x"]').count()).toBe(0)
  hold = true
  await page
    .getByRole('button', { name: 'Validate and submit', exact: true })
    .click()
  await expect.poll(() => enteredHeldValidation).toBe(true)
  epoch = 5
  await expect(page.getByRole('alert')).toContainText('lost control', {
    timeout: 7000,
  })
  await expect(
    page.getByRole('button', { name: 'Validate and submit', exact: true }),
  ).toBeDisabled()
  releaseValidation()
  await expect(page.locator('.console-decision [role="status"]')).toContainText(
    'submit cancelled',
  )
  expect(submits).toHaveLength(1)
  expect(
    routes.some(
      (p) =>
        p.includes('/capabilities') ||
        p.includes('/controller') ||
        p.endsWith('/resume') ||
        p.includes('commonwealth'),
    ),
  ).toBe(false)
  expect(errors).toEqual([])
})
test('missing fragment or operator capability never reaches seat data', async ({
  page,
}) => {
  const calls: string[] = []
  await page.route('**/api/**', (r) => {
    calls.push(new URL(r.request().url()).pathname)
    return r.fulfill({
      json: { operator: true, perspective: 'operator', campaign_id: null },
    })
  })
  await page.goto('/console.html')
  await expect(page.getByRole('alert')).toContainText('fresh link')
  expect(calls).toEqual([])
  await page.goto(`/console.html?new-document=1#cap=${token}`)
  await expect(page.getByRole('alert')).toContainText(
    'requires a seat capability',
  )
  expect(calls).toEqual(['/api/session'])
})
