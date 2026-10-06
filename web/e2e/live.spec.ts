import { expect, test } from '@playwright/test'
import type { ServerMessage, Subscribe } from '../src/protocol'
test('uses the real adapter, renders objectives, and replaces server projections', async ({
  page,
}) => {
  const errors: string[] = [],
    requests: Subscribe[] = []
  await page.route('**/api/session', (route) => {
    expect(route.request().headers().authorization).toBe(
      `Bearer ${'a'.repeat(64)}`,
    )
    return route.fulfill({
      json: { operator: true, campaign_id: null, perspective: 'operator' },
    })
  })
  let releaseProjection: (() => void) | undefined
  let denyProjection: (() => void) | undefined
  page.on('pageerror', (e) => errors.push(e.message))
  await page.route('**/api/campaigns/fixture?perspective=*', (route) =>
    route.fulfill({ json: { status: { state: 'paused' } } }),
  )
  await page.routeWebSocket(
    '**/api/campaigns/fixture/stream?cap=*',
    (socket) => {
      denyProjection = () => {
        void socket.close({ code: 1008, reason: 'hidden internal reason' })
      }
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
              pending: own
                ? [
                    {
                      id: 'fixture-initiative',
                      seat: 'axis.commander',
                      kind: 'initiative',
                      summary: 'Choose Player A',
                      opened_seq: 0,
                      rules: ['land:7.11'],
                    },
                  ]
                : [],
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
        const send = () =>
          messages.forEach((message) => socket.send(JSON.stringify(message)))
        if (request.perspective === 'side:commonwealth')
          releaseProjection = send
        else send()
      })
    },
  )
  await page.goto(`/?campaign=fixture#cap=${'a'.repeat(64)}`)
  await expect(page).toHaveURL(/campaign=fixture$/)
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
  await expect(page.getByLabel('Rule citations')).toHaveText('land:7.11')
  async function counterPixels() {
    const png = await page.locator('.board canvas').screenshot()
    return page.evaluate(async (base64) => {
      const image = new Image()
      image.src = `data:image/png;base64,${base64}`
      await image.decode()
      const canvas = document.createElement('canvas')
      canvas.width = image.width
      canvas.height = image.height
      const context = canvas.getContext('2d')!
      context.drawImage(image, 0, 0)
      const pixels = context.getImageData(0, 0, image.width, image.height).data
      let count = 0
      for (let i = 0; i < pixels.length; i += 4)
        if (pixels[i] === 219 && pixels[i + 1] === 200 && pixels[i + 2] === 160)
          count++
      return count
    }, png.toString('base64'))
  }
  await expect(page.getByTestId('fps')).toHaveText(/^[1-9]\d* FPS/)
  expect(await counterPixels()).toBeGreaterThan(20)
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect.poll(() => Boolean(releaseProjection)).toBeTruthy()
  await expect(page.getByTestId('playback-status')).toHaveText('CONNECTING')
  expect(await counterPixels()).toBe(0)
  await expect(page.getByLabel('Rule citations')).toHaveCount(0)
  releaseProjection!()
  await expect(page.locator('.unit-row')).toHaveCount(0)
  await expect(page.locator('.inspector')).toContainText(
    'Presence disclosed; composition unavailable',
  )
  expect(requests.map((r) => r.perspective)).toEqual([
    'operator',
    'side:commonwealth',
  ])
  await expect(
    page.getByRole('button', { name: 'Resume campaign' }),
  ).toBeDisabled()
  await page.screenshot({
    path: '../../board-adapter-fixture.png',
    fullPage: true,
  })
  await page.getByLabel('Perspective', { exact: true }).selectOption('operator')
  await expect(page.locator('.unit-row')).toHaveCount(1)
  await expect(page.locator('.transcript-scroll')).toContainText(
    'Fixture streamed session',
  )
  denyProjection!()
  await expect(page.locator('.campaign-title')).toContainText('does not allow')
  await expect(page.locator('.unit-row')).toHaveCount(0)
  await expect(page.locator('.transcript-scroll')).toHaveCount(0)
  await expect(page.locator('body')).not.toContainText('hidden internal reason')
  expect(errors).toEqual([])
})
