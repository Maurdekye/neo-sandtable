import { expect, test } from '@playwright/test'
import type { Clock, SeatInfo, ServerMessage, Subscribe } from '../src/protocol'
import { writeFileSync } from 'node:fs'
const clock: Clock = {
  game_turn: 1,
  op_stage: 1,
  date: '1940-09-15',
  stage: 'opstage',
  phase: 'movement_and_combat',
  segment: 'movement',
  step: null,
  phasing: 'axis',
}
test('shows ten fixture seats, waiting, literal explanation, errors and polled handover', async ({
  page,
}) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  const seats: SeatInfo[] = Array.from({ length: 10 }, (_, i) => ({
    id: `axis.seat${i}`,
    side: 'axis',
    role: 'commander',
    controller: {
      kind: i === 9 ? 'scripted' : 'llm-cli',
      label: i === 9 ? 'legal_random' : 'Synthetic Claude / haiku',
    },
    status: i === 1 ? 'paused' : 'deciding',
  }))
  const usage = {
    kind: 'usage_snapshot' as const,
    controller_epoch: 1,
    revision: 1,
    provider: 'Synthetic Claude',
    model: 'haiku fixture',
    attempts: 3,
    completed: 2,
    input_tokens: 100,
    output_tokens: 20,
    cache_read_tokens: 50,
    cache_creation_tokens: null,
    reasoning_tokens: null,
    reported_cost_usd: 0.003,
    incomplete_turns: 1,
  }
  let epoch = 1
  await page.route('**/api/session', (r) =>
    r.fulfill({
      json: { operator: true, campaign_id: null, perspective: 'operator' },
    }),
  )
  await page.route('**/api/campaigns/monitor-fixture?perspective=*', (r) =>
    r.fulfill({ json: { status: { state: 'paused' } } }),
  )
  await page.route(
    '**/api/campaigns/monitor-fixture/seats?perspective=*',
    (r) => r.fulfill({ json: seats }),
  )
  await page.route('**/api/campaigns/monitor-fixture/seats/*/observe', (r) =>
    r.fulfill({
      json: {
        paused: r.request().url().includes('seat1/'),
        failure: r.request().url().includes('seat1/')
          ? 'Synthetic driver quota pause'
          : null,
        controller_epoch: epoch,
      },
    }),
  )
  await page.routeWebSocket(
    '**/api/campaigns/monitor-fixture/stream?cap=*',
    (socket) => {
      const send = (m: ServerMessage) => socket.send(JSON.stringify(m))
      socket.onMessage((data) => {
        const sub = JSON.parse(String(data)) as Subscribe
        send({
          type: 'hello',
          protocol: 1,
          perspective: sub.perspective,
          campaign: {
            id: 'monitor-fixture',
            title: 'Synthetic ten-seat monitoring',
            scenario_id: 'fixture',
            rules_profile: 'fixture',
            seats,
          },
        })
        send({
          type: 'snapshot',
          seq: 0,
          view: {
            clock,
            stacks: [],
            units: {},
            markers: [],
            pending: [
              {
                id: 'wait',
                seat: 'axis.seat2',
                kind: 'movement.orders',
                summary: 'Synthetic movement order',
                rules: [],
                opened_seq: 0,
              },
            ],
          },
        })
        send({
          type: 'event',
          seq: 1,
          clock,
          event: {
            kind: 'decision_resolved',
            seat: 'axis.seat0',
            decision_id: 'accepted',
            summary: 'Synthetic accepted coast order',
            explanation:
              '<img src=x onerror=alert(1)> **literal** https://example.test',
          },
        })
        send({
          type: 'transcript',
          seat: 'axis.seat0',
          tseq: 1,
          game_seq: 1,
          at: '2026-10-07T06:00:00Z',
          entry: {
            kind: 'decision_submitted',
            decision_id: 'accepted',
            summary: 'Synthetic accepted coast order',
          },
        })
        for (const [i, revision] of [1, 2, 1].entries())
          send({
            type: 'transcript',
            seat: 'axis.seat0',
            tseq: i + 2,
            game_seq: 1,
            at: '2026-10-07T06:00:02Z',
            entry: {
              ...usage,
              revision,
              input_tokens: revision === 2 ? 200 : 100,
            },
          })
        send({
          type: 'transcript',
          seat: 'axis.seat4',
          tseq: 1,
          game_seq: 1,
          at: '2026-10-07T06:00:02Z',
          entry: {
            ...usage,
            input_tokens: null,
            output_tokens: 0,
            reported_cost_usd: null,
          },
        })
        send({
          type: 'transcript',
          seat: 'axis.seat3',
          tseq: 1,
          game_seq: 1,
          at: '2026-10-07T06:00:01Z',
          entry: {
            kind: 'tool_result',
            call_id: 'failed',
            ok: false,
            summary: 'Synthetic validation failed',
            detail: null,
          },
        })
      })
    },
  )
  await page.goto('/?campaign=monitor-fixture#cap=' + 'a'.repeat(64))
  await expect(page.locator('.seat-card')).toHaveCount(10)
  await expect(page.getByTestId('answer-count')).toHaveText(
    '1 answers received',
  )
  const card = page.locator('.seat-card[data-seat="axis.seat0"]')
  await card.locator('summary').click()
  await expect(card).toContainText(
    '<img src=x onerror=alert(1)> **literal** https://example.test',
  )
  expect(await card.locator('img,a').count()).toBe(0)
  await expect(card).toContainText(
    'Input 200 / output 20 / USD $0.003 / incomplete',
  )
  await expect(card).toContainText(
    'Cache read: 50 / cache creation: not reported',
  )
  await expect(
    page.locator('.seat-card[data-seat="axis.seat4"]'),
  ).toContainText(
    'Input not reported / output 0 / USD not reported / incomplete',
  )
  await page.screenshot({ path: '../../board-ten-seat-usage-fixture.png' })
  await expect(
    page.locator('.seat-card[data-seat="axis.seat2"]'),
  ).toContainText('observed')
  const paused = page.locator('.seat-card[data-seat="axis.seat1"]')
  await paused.locator('summary').click()
  await expect(paused).toContainText('Synthetic driver quota pause')
  await expect(
    page.locator('.seat-card[data-seat="axis.seat9"]'),
  ).toContainText('scripted, no usage')
  epoch = 2
  await expect(paused).toContainText('handover observed', { timeout: 20000 })
  await expect(card).toContainText('Tokens: not reported / USD: not reported')
  await page.screenshot({ path: '../../board-ten-seat-monitor-fixture.png' })
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  await expect(
    page.getByRole('region', { name: 'AI play monitoring' }),
  ).toHaveCount(0)
  expect(errors).toEqual([])
})
const server = process.env.CNA_SMOKE_SERVER,
  cap = process.env.CNA_SMOKE_CAPABILITY
const headers = { Authorization: `Bearer ${cap ?? ''}` }
test('watches authentic scripted Graziani acceptances with no reported usage', async ({
  page,
  request,
}) => {
  test.skip(!server || !cap, 'Requires own authenticated smoke server')
  test.setTimeout(120000)
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  const created = await request.post(`${server}/api/campaigns`, {
    headers,
    data: {
      kind: 'cna',
      rules_profile: 'cna-2021-dev',
      title: 'Graziani - scripted seat monitoring',
      seed: Array(32).fill(0),
      paused: true,
      controller: 'legal_random',
    },
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json(),
    base = `${server}/api/campaigns/${meta.id}`
  try {
    await page.addInitScript(
      ({ server, cap }) =>
        sessionStorage.setItem(`cna:cap:${new URL(server).origin}`, cap),
      { server: server!, cap: cap! },
    )
    await page.goto(`${server}/?campaign=${meta.id}`)
    await expect(page.locator('.seat-card')).toHaveCount(meta.seats.length)
    await expect(page.locator('.seat-strip')).toContainText(
      'scripted, no usage',
    )
    expect(
      (await request.post(`${base}/resume`, { headers })).ok(),
    ).toBeTruthy()
    await expect(page.getByTestId('answer-count')).not.toHaveText(
      '0 answers received',
      { timeout: 60000 },
    )
    expect((await request.post(`${base}/pause`, { headers })).ok()).toBeTruthy()
    const details = page.locator('.seat-card details')
    for (let i = 0; i < (await details.count()); i++)
      await details.nth(i).evaluate((el) => el.setAttribute('open', ''))
    await expect(page.locator('.seat-strip')).toContainText('Accepted:')
    await page.screenshot({ path: '../../board-graziani-seat-monitor.png' })
    writeFileSync(
      '../../seat-monitor-browser-verification.json',
      JSON.stringify(
        {
          commit: process.env.CNA_SMOKE_COMMIT,
          campaign: meta.id,
          kind: 'cna',
          controller: 'legal_random',
          seats: meta.seats.length,
          answers: await page.getByTestId('answer-count').innerText(),
          errors,
          paid_calls: 0,
          usage: 'scripted, no usage',
          screenshot: 'board-graziani-seat-monitor.png',
        },
        null,
        2,
      ),
    )
    expect(errors).toEqual([])
  } finally {
    await request.post(`${base}/pause`, { headers })
  }
})
