import { expect, test } from '@playwright/test'
import type { ServerMessage, Subscribe, UnitView } from '../src/protocol'
test('presents authorized coast movement, locators, combat citations and private status', async ({
  page,
}) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.route('**/api/session', (r) =>
    r.fulfill({
      json: { operator: true, campaign_id: null, perspective: 'operator' },
    }),
  )
  await page.route('**/api/campaigns/coast-fixture?perspective=*', (r) =>
    r.fulfill({ json: { status: { state: 'paused' } } }),
  )
  const clock = {
    game_turn: 1,
    date: '1940-09-15',
    stage: 'opstage',
    phase: 'movement_and_combat',
    segment: 'movement',
    step: null,
    op_stage: 1,
    phasing: 'axis' as const,
  }
  const unit: UnitView = {
    id: 'coast-fixture-unit',
    name: 'Synthetic coast battalion',
    side: 'axis',
    hex: 'C4120',
    kind: 'artillery',
    size: 'battalion',
    nationality: 'italian',
    parent: null,
    detail: {
      engaged: true,
      combat_pinned: true,
      moved_this_segment: true,
      gun_position: 'deployed',
      reserve: { status: 'first', released_for_cycle: 1 },
      cp_spent_quarters: 5,
    },
  }
  const explanation =
    '<img src=x onerror=alert(1)> https://example.test **literal**'
  let traffic: (() => void) | undefined
  let publish: (() => void) | undefined
  await page.routeWebSocket(
    '**/api/campaigns/coast-fixture/stream?cap=*',
    (socket) => {
      const send = (m: ServerMessage) => socket.send(JSON.stringify(m))
      socket.onMessage((data) => {
        const req = JSON.parse(String(data)) as Subscribe,
          own = req.perspective !== 'side:commonwealth'
        send({
          type: 'hello',
          protocol: 1,
          perspective: req.perspective,
          campaign: {
            id: 'coast-fixture',
            title: 'Synthetic coast presentation fixture',
            rules_profile: 'fixture',
            scenario_id: 'fixture',
            seats: [
              {
                id: 'axis.front_line',
                side: 'axis',
                role: 'front_line',
                controller: null,
                status: 'idle',
              },
            ],
          },
        })
        send({
          type: 'snapshot',
          seq: 0,
          view: {
            clock,
            units: own ? { [unit.id]: unit } : {},
            stacks: [
              {
                hex: 'C4120',
                side: 'axis',
                unit_ids: own ? [unit.id] : [],
                visible_count: own ? 1 : null,
              },
            ],
            markers: [],
            pending: [],
          },
        })
        traffic = () => {
          if (!own) return
          for (let seq = 6; seq <= 620; seq++)
            send({
              type: 'event',
              seq,
              clock,
              event: { kind: 'note', text: 'Synthetic background traffic' },
            })
        }
        publish = () => {
          if (!own) return
          send({
            type: 'event',
            seq: 1,
            clock,
            event: {
              kind: 'unit_moved',
              unit_id: unit.id,
              path: ['C4220'],
              cp_spent: 1.25,
            },
          })
          send({
            type: 'event',
            seq: 2,
            clock,
            hex: 'C4220',
            unit_id: unit.id,
            event: {
              kind: 'note',
              text: 'Synthetic stop: blocked by newly disclosed control.',
            },
          })
          send({
            type: 'event',
            seq: 3,
            clock,
            hex: 'C4220',
            event: {
              kind: 'dice_rolled',
              purpose: 'Synthetic barrage',
              dice: [2, 5],
              reading: 25,
              rule: 'land:12.1',
            },
          })
          send({
            type: 'event',
            seq: 4,
            clock,
            event: {
              kind: 'unit_removed',
              unit_id: 'absent-unit',
              reason: 'Synthetic retreat elimination',
            },
          })
          send({
            type: 'event',
            seq: 5,
            clock,
            event: {
              kind: 'decision_resolved',
              seat: 'axis.front_line',
              decision_id: 'opaque',
              summary: 'Synthetic accepted decision',
              explanation,
            },
          })
          send({
            type: 'transcript',
            seat: 'axis.front_line',
            tseq: 1,
            game_seq: 5,
            at: '1940-09-15T00:00:00Z',
            entry: {
              kind: 'decision_submitted',
              decision_id: 'opaque',
              summary: 'Synthetic decision',
            },
          })
        }
      })
    },
  )
  await page.goto(`/?campaign=coast-fixture#cap=${'b'.repeat(64)}`)
  await expect(page.locator('.board canvas')).toBeVisible()
  await page.getByLabel('Terrain classification & corridor').check()
  await page.getByLabel('Find formation or unit').fill(unit.id)
  await page.locator(`.formation-unit[data-unit-id="${unit.id}"]`).click()
  await expect(
    page.locator('.unit-detail [data-testid="unit-status"]'),
  ).toContainText('Engaged')
  await expect(
    page.locator('.unit-detail [data-testid="unit-status"]'),
  ).toContainText('1.25 CP spent')
  const animation = page.waitForFunction(
    () =>
      Number(
        document
          .querySelector('[data-testid="motion-count"]')
          ?.textContent?.split(' ')[0],
      ) > 0,
  )
  publish!()
  await animation
  await expect(
    page.locator('.event-row[data-kind="unit_moved"]'),
  ).toContainText('1.25 CP spent')
  const note = page.locator('.event-row[data-kind="note"]')
  await note.getByRole('button', { name: 'Locate C4220' }).click()
  await expect(page.getByTestId('terrain-classification')).toContainText(
    'digitization corridor',
  )
  await expect(
    page.locator('.event-row[data-kind="unit_removed"]'),
  ).toContainText('Unlocated')
  await expect(
    page.locator('.event-row[data-kind="unit_removed"] button'),
  ).toHaveCount(0)
  await page.locator('.event-row[data-kind="dice_rolled"] .citation').hover()
  await expect(page.getByRole('tooltip')).toContainText('land:12.1')
  await page.mouse.move(800, 100)
  const commentary = page.locator('.sessions .decision-explanation')
  await expect(commentary).toContainText(explanation)
  await expect(commentary.locator('img,a,strong')).toHaveCount(0)
  await page.getByTestId('stage-overview').locator(':scope > summary').click()
  await page
    .locator('.stage-category > summary')
    .filter({ hasText: 'Seat decisions' })
    .click()
  const timelineCommentary = page.locator(
    '.stage-overview .decision-explanation',
  )
  await expect(timelineCommentary).toContainText(explanation)
  await expect(timelineCommentary.locator('img,a,strong')).toHaveCount(0)
  traffic!()
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(page.locator('.sequence')).toContainText('620')
  await page.locator('.stage-move').first().click()
  await expect(page.getByTestId('playback-status')).toContainText('HISTORY')
  await expect(page.getByTestId('archive-caption')).toContainText(
    'outside retained',
  )
  await expect(page.getByRole('button', { name: /^Step/ })).toBeDisabled()
  await expect(
    page.locator('.event-row[data-kind="unit_moved"]'),
  ).toContainText('1.25 CP spent')
  await page.getByRole('button', { name: 'Return to live' }).click()

  await page.screenshot({
    path: '../../board-coast-fixture.png',
    fullPage: true,
  })
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(page.locator('.stage-move')).toHaveCount(0)
  await expect(page.locator('.decision-explanation')).toHaveCount(0)
  await expect(page.locator('.status-badges')).toHaveCount(0)
  await expect(page.locator('.event-row')).toHaveCount(0)
  await expect(page.getByTestId('motion-count')).toContainText('0 active')
  expect(errors).toEqual([])
})
