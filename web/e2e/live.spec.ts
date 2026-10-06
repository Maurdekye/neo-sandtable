import { expect, test } from '@playwright/test'
import type { ServerMessage, Subscribe } from '../src/protocol'
test('uses the real adapter, renders objectives, and replaces server projections', async ({
  page,
}) => {
  const errors: string[] = [],
    requests: Subscribe[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.routeWebSocket('**/api/campaigns/fixture/stream', (socket) => {
    socket.onMessage((data) => {
      const request = JSON.parse(String(data)) as Subscribe
      requests.push(request)
      const own =
        request.perspective === 'operator' ||
        request.perspective === 'side:axis'
      const clock = {
        game_turn: 1,
        date: '1940-09-15',
        stage: 'opstage',
        op_stage: 1,
        phase: 'movement_and_combat',
        segment: null,
        step: null,
        phasing: 'axis' as const,
      }
      const messages: ServerMessage[] = [
        {
          type: 'hello',
          protocol: 1,
          perspective: request.perspective,
          campaign: {
            id: 'fixture',
            scenario_id: 'sandbox',
            rules_profile: 'fixture',
            title: 'Adapter fixture',
            seats: [
              {
                id: 'axis.commander',
                side: 'axis',
                role: 'commander',
                controller: { kind: 'llm-cli', label: 'Fixture seat' },
                status: 'deciding',
              },
            ],
          },
        },
        {
          type: 'snapshot',
          seq: 0,
          view: {
            clock,
            stacks: [
              {
                hex: 'C4218',
                side: 'axis',
                unit_ids: own ? ['u'] : [],
                visible_count: own ? 1 : null,
              },
            ],
            units: own
              ? {
                  u: {
                    id: 'u',
                    name: 'Fixture formation',
                    hex: 'C4218',
                    side: 'axis',
                    nationality: 'italian',
                    kind: 'infantry',
                    size: 'division',
                    parent: null,
                    detail: null,
                  },
                }
              : {},
            markers: [
              {
                id: 'obj',
                kind: 'objective',
                hex: 'C4218',
                side: null,
                label: 'Fixture objective',
              },
            ],
            pending: [],
          },
        },
        {
          type: 'event',
          seq: 1,
          clock,
          event: { kind: 'note', text: 'Fixture event received' },
        },
        {
          type: 'transcript',
          seat: 'axis.commander',
          tseq: 1,
          game_seq: 1,
          at: '2026-10-06T00:00:00Z',
          entry: { kind: 'assistant_text', text: 'Fixture streamed session' },
        },
      ]
      messages.forEach((message) => socket.send(JSON.stringify(message)))
    })
  })
  await page.goto('/?campaign=fixture')
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await expect(page.locator('.campaign-title')).toContainText(
    'Live server stream',
  )
  await expect(page.locator('.board canvas')).toBeVisible()
  await expect(page.locator('.transcript-scroll')).toContainText(
    'Fixture streamed session',
  )
  await expect(
    page.getByText('LIVE TRANSCRIPTS', { exact: true }),
  ).toBeVisible()
  await page.getByRole('button', { name: 'Fixture objective' }).click()
  await expect(page.locator('.inspector')).toContainText('holder: unheld')
  await expect(page.locator('.unit-row')).toHaveCount(1)
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(page.locator('.unit-row')).toHaveCount(0)
  await expect(page.locator('.inspector')).toContainText(
    'Presence disclosed; composition unavailable',
  )
  expect(requests.map((r) => r.perspective)).toEqual([
    'operator',
    'side:commonwealth',
  ])
  await expect(
    page.getByRole('button', { name: 'Campaign control pending API' }),
  ).toBeDisabled()
  await page.screenshot({
    path: '../../board-adapter-fixture.png',
    fullPage: true,
  })
  expect(errors).toEqual([])
})
