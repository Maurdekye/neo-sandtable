import { expect, test } from '@playwright/test'
import type { ServerMessage, Subscribe, UnitView } from '../src/protocol'
test('renders projected setup areas and clears private choices on perspective switch', async ({
  page,
}) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.route('**/api/session', (r) =>
    r.fulfill({
      json: { operator: true, campaign_id: null, perspective: 'operator' },
    }),
  )
  await page.route('**/api/campaigns/setup-fixture?perspective=*', (r) =>
    r.fulfill({ json: { status: { state: 'paused' } } }),
  )
  const clock = {
    game_turn: 1,
    date: '1940-09-15',
    stage: 'setup',
    phase: 'placement',
    op_stage: null,
    segment: null,
    step: null,
    phasing: null,
  }
  const unit: UnitView = {
    id: 'fixture-awaiting',
    name: 'Fixture awaiting battalion',
    hex: null,
    side: 'axis',
    kind: 'infantry',
    size: 'battalion',
    nationality: 'italian',
    parent: null,
    detail: { location: { at: 'awaiting_setup', group: 'fixture-group' } },
  }
  let reveal: (() => void) | undefined
  await page.routeWebSocket(
    '**/api/campaigns/setup-fixture/stream?cap=*',
    (socket) => {
      const send = (m: ServerMessage) => socket.send(JSON.stringify(m))
      socket.onMessage((data) => {
        const request = JSON.parse(String(data)) as Subscribe,
          own = request.perspective !== 'side:commonwealth'
        send({
          type: 'hello',
          protocol: 1,
          perspective: request.perspective,
          campaign: {
            id: 'setup-fixture',
            title: 'Synthetic setup transport fixture',
            scenario_id: 'fixture',
            rules_profile: 'fixture',
            seats: [],
          },
        })
        send({
          type: 'snapshot',
          seq: 0,
          view: {
            clock,
            stacks: [],
            markers: [],
            units: own ? { [unit.id]: unit } : {},
            pending: own
              ? [
                  {
                    id: 'opaque-setup-id',
                    seat: 'axis.commander',
                    kind: 'cna.setup.unit',
                    summary: 'Fixture placement',
                    opened_seq: 0,
                    rules: ['scen:60.31'],
                    space: {
                      type: 'string',
                      enum: ['C4218', 'C4219', 'box_tripoli'],
                      'x-context': { unit: unit.id, group: 'fixture-group' },
                    },
                  },
                ]
              : [],
          },
        })
        reveal = () => {
          send({
            type: 'event',
            seq: 1,
            clock,
            event: {
              kind: 'decision_resolved',
              decision_id: 'opaque-setup-id',
              seat: 'axis.commander',
              summary: 'Fixture window closed',
            },
          })
          send({
            type: 'event',
            seq: 2,
            clock,
            event: own
              ? {
                  kind: 'unit_updated',
                  unit: { ...unit, hex: 'C4218', detail: {} },
                }
              : {
                  kind: 'stack_updated',
                  stack: {
                    hex: 'C4218',
                    side: 'axis',
                    unit_ids: [],
                    visible_count: null,
                  },
                },
          })
        }
      })
    },
  )
  await page.goto(`/?campaign=setup-fixture#cap=${'b'.repeat(64)}`)
  await expect(page.locator('.board canvas')).toBeVisible()
  await expect(page.locator('.setup-unit')).toContainText(unit.name)
  await page.locator('.show-placement').click()
  await expect(page.getByTestId('placement-highlight')).toContainText(
    '2 legal set-up hexes',
  )
  await page.locator('.setup-unit').click()
  await expect(page.locator('.unit-detail')).toContainText('Awaiting setup')
  await page.locator('.setup-destinations summary').click()
  await expect(page.locator('.setup-destinations')).toContainText('box_tripoli')
  await expect(
    page
      .locator('.setup-destinations button')
      .filter({ hasText: 'box_tripoli' }),
  ).toBeDisabled()
  const animation = page.waitForFunction(
    () =>
      Number(
        document
          .querySelector('[data-testid="motion-count"]')
          ?.textContent?.split(' ')[0],
      ) > 0,
  )
  reveal!()
  await animation
  await expect(page.getByTestId('placement-highlight')).toHaveCount(0)
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(page.locator('.placement-decision')).toHaveCount(0)
  await expect(page.getByTestId('placement-highlight')).toHaveCount(0)
  await expect(page.locator('.map-error')).toHaveCount(0)
  expect(errors).toEqual([])
})
