import { expect, test, type Page } from '@playwright/test'
import { writeFileSync } from 'node:fs'
import type { ServerMessage, UnitView } from '../src/protocol'
const server = process.env.CNA_SMOKE_SERVER,
  operator = process.env.CNA_SMOKE_CAPABILITY
// The trusted test harness creates/binds a game. Neither browser page receives this operator token.
test('two isolated human tabs answer real Graziani setup and a coast movement order', async ({
  browser,
  request,
}) => {
  test.skip(
    !server || !operator,
    'Set fresh no-paid authenticated smoke server',
  )
  // Manual slow proof: completed 1.6 min (1.9 min including startup) on 2026-10-07.
  // Earlier setup-only observations took up to 2.6 min; keep a bounded 2x ceiling.
  test.setTimeout(360000)
  const opHeaders = { Authorization: `Bearer ${operator}` },
    created = await request.post(`${server}/api/campaigns`, {
      headers: opHeaders,
      data: {
        kind: 'cna',
        rules_profile: 'cna-2021-dev',
        seed: Array(32).fill(0),
        title: 'Human console browser proof',
        paused: true,
        controller: 'legal_random',
      },
    })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json(),
    base = `${server}/api/campaigns/${meta.id}`
  const capabilities = await (
      await request.get(`${base}/capabilities`, { headers: opHeaders })
    ).json(),
    seats = ['axis.commander', 'axis.front_line']
  for (const seat of seats)
    expect(
      (
        await request.post(`${base}/seats/${seat}/controller`, {
          headers: opHeaders,
          data: {
            controller: { kind: 'human', label: 'Human browser proof' },
            config: {},
          },
        })
      ).ok(),
    ).toBeTruthy()
  const contexts = await Promise.all(seats.map(() => browser.newContext())),
    pages = await Promise.all(contexts.map((c) => c.newPage())),
    errors: string[] = [],
    calls: { seat: string; path: string }[] = [],
    units = new Map<string, UnitView>(),
    moved: string[] = []
  try {
    for (const [i, page] of pages.entries()) {
      const seat = seats[i],
        cap = capabilities.seats[seat]
      await page.addInitScript(() => {
        Object.defineProperty(Storage.prototype, 'setItem', {
          value: () => {
            throw new Error('Human console wrote storage')
          },
        })
        Object.defineProperty(Storage.prototype, 'getItem', {
          value: () => {
            throw new Error('Human console read storage')
          },
        })
      })
      page.on('pageerror', (e) => errors.push(e.message))
      page.on('request', (r) => {
        const url = new URL(r.url())
        if (url.pathname.startsWith('/api/')) {
          calls.push({ seat, path: url.pathname })
          expect(r.headers().authorization === `Bearer ${cap}`).toBe(true)
          expect(
            url.pathname === '/api/session' ||
              url.pathname.startsWith(
                `/api/campaigns/${meta.id}/seats/${seat}/`,
              ),
          ).toBe(true)
        }
      })
      page.on('websocket', (socket) =>
        socket.on('framereceived', (frame) => {
          if (typeof frame.payload !== 'string') return
          const m = JSON.parse(frame.payload) as ServerMessage
          if (i === 1) {
            if (m.type === 'snapshot')
              Object.values(m.view.units).forEach((u) => units.set(u.id, u))
            if (m.type === 'event' && m.event.kind === 'unit_updated')
              units.set(m.event.unit.id, m.event.unit)
            if (m.type === 'event' && m.event.kind === 'unit_moved')
              moved.push(m.event.unit_id)
          }
        }),
      )
      await page.goto(
        `${server}/console.html?campaign=${meta.id}&seat=${seat}#cap=${cap}`,
      )
      await expect(page.getByRole('heading', { level: 1 })).toHaveText(seat)
      expect(!page.url().includes(cap)).toBe(true)
      const other = seats[1 - i],
        headers = { Authorization: `Bearer ${cap}` }
      for (const path of [
        `${base}/capabilities`,
        `${base}/seats/${other}/observe`,
      ])
        expect((await request.get(path, { headers })).status()).toBe(403)
      expect(
        (
          await request.post(
            `${base}/seats/${other}/decisions/not-owned/submit`,
            {
              headers,
              data: {
                decision_id: 'not-owned',
                seat: other,
                controller_epoch: 1,
                decision_revision: 0,
                idempotency_key: 'cross-denied',
                action: null,
                public_explanation: null,
              },
            },
          )
        ).status(),
      ).toBe(403)
    }
    expect(
      (await request.post(`${base}/resume`, { headers: opHeaders })).ok(),
    ).toBeTruthy()
    let placements = 0,
      movement: { id: string; revision: number; summary: string } | undefined
    const observe = async (seat: string) =>
      await (
        await request.get(`${base}/seats/${seat}/observe`, {
          headers: { Authorization: `Bearer ${capabilities.seats[seat]}` },
        })
      ).json()
    const answer = async (page: Page) => {
      await expect(
        page.getByRole('button', { name: 'Validate and submit', exact: true }),
      ).toBeEnabled()
      const result = page.waitForResponse(
        (r) => r.url().endsWith('/submit') && r.request().method() === 'POST',
      )
      await page
        .getByRole('button', { name: 'Validate and submit', exact: true })
        .click()
      expect((await result).ok()).toBeTruthy()
    }
    const deadline = Date.now() + 300000
    while (Date.now() < deadline) {
      const front = await observe(seats[1])
      movement = front.pending.find(
        (d: { kind: string }) => d.kind === 'cna.movement.orders',
      )
      if (movement) break
      if (front.pending.length) {
        const pending = front.pending[0]
        console.log(`Answering own front_line prerequisite: ${pending.kind}`)
        await expect(pages[1].locator('.console-decision')).toContainText(
          pending.summary,
        )
        await answer(pages[1])
        continue
      }
      const commander = await observe(seats[0]),
        pending = commander.pending[0]
      if (pending && placements === 0) {
        await expect(pages[0].locator('.console-decision')).toContainText(
          pending.summary,
        )
        if (pending.kind === 'cna.setup.unit') {
          placements++
          if (placements === 1) {
            await pages[0]
              .getByRole('button', { name: 'Pick hex on map', exact: true })
              .click()
            await expect(
              pages[0].getByTestId('placement-highlight'),
            ).toContainText('enumerated target hexes')
            await pages[0].screenshot({
              path: '../../board-human-setup.png',
              fullPage: true,
            })
            await pages[0]
              .getByRole('button', { name: 'Stop picking', exact: true })
              .click()
          }
        }
        await answer(pages[0])
        if (placements === 1) {
          expect(
            (
              await request.post(`${base}/seats/${seats[0]}/controller`, {
                headers: opHeaders,
                data: {
                  controller: { kind: 'scripted', label: 'legal_random' },
                  config: { mode: 'legal_random' },
                },
              })
            ).ok(),
          ).toBeTruthy()
          await expect(pages[0].getByRole('alert')).toContainText(
            'lost control',
            { timeout: 7000 },
          )
        }
      } else await pages[0].waitForTimeout(750)
    }
    if (!movement) {
      const status = await (
        await request.get(`${base}?perspective=operator`, {
          headers: opHeaders,
        })
      ).json()
      console.log(
        JSON.stringify({
          status: status.status,
          clock: status.snapshot?.view?.clock,
          pending: status.snapshot?.view?.pending?.map(
            (d: { seat: string; kind: string; summary: string }) => ({
              seat: d.seat,
              kind: d.kind,
              summary: d.summary,
            }),
          ),
        }),
      )
    }
    expect(placements).toBeGreaterThan(0)
    expect(
      movement,
      'scripted peers must reach human front_line movement',
    ).toBeTruthy()
    if (!movement) throw new Error('Human movement window did not open')
    const hydrated = await (
        await request.get(
          `${base}/seats/${seats[1]}/decisions/${movement.id}/actions`,
          {
            headers: {
              Authorization: `Bearer ${capabilities.seats[seats[1]]}`,
            },
          },
        )
      ).json(),
      list = hydrated.action_schema.anyOf.find(
        (s: { type?: string }) => s.type === 'array',
      ),
      ids: string[] = list.items.properties.unit.enum
    let order: { unit: string; path: string[] } | undefined
    for (const id of ids) {
      if (units.get(id)?.hex !== 'C4120') continue
      const candidate = { unit: id, path: ['C4220'] }
      const valid = await request.post(
        `${base}/seats/${seats[1]}/decisions/${movement.id}/validate`,
        {
          headers: { Authorization: `Bearer ${capabilities.seats[seats[1]]}` },
          data: {
            action: [candidate],
            controller_epoch: (await observe(seats[1])).controller_epoch,
            decision_revision: movement.revision,
          },
        },
      )
      if (valid.ok()) {
        order = candidate
        break
      }
    }
    expect(order, 'find an engine-validated own Cirene road move').toBeTruthy()
    const front = pages[1]
    await expect(front.locator('.console-decision')).toContainText(
      movement.summary,
    )
    await front.getByRole('checkbox', { name: 'Pass', exact: true }).uncheck()
    await front.getByRole('button', { name: 'Add Action', exact: true }).click()
    await front.getByLabel('unit', { exact: true }).selectOption(order!.unit)
    await front.getByLabel('path 1', { exact: true }).fill(order!.path[0])
    await front
      .getByLabel('Seat commentary')
      .fill('Human browser harness: one verified coast road step.')
    await answer(front)
    await expect
      .poll(() => moved.includes(order!.unit), { timeout: 15000 })
      .toBe(true)
    await front.getByText('Own seat event feed', { exact: true }).click()
    await front
      .locator('.event-row[data-kind="unit_moved"][data-hex="C4220"]')
      .getByRole('button', { name: 'Locate C4220', exact: true })
      .click()
    await expect(front.locator('.console-units')).toContainText(
      units.get(order!.unit)!.name,
    )
    await front.screenshot({
      path: '../../board-human-movement.png',
      fullPage: true,
    })
    expect(errors).toEqual([])
    writeFileSync(
      '../../human-console-browser-verification.json',
      JSON.stringify(
        {
          commit: process.env.CNA_SMOKE_COMMIT,
          campaign: meta.id,
          seats,
          placements,
          order,
          cross_read_denied: true,
          cross_submit_denied: true,
          no_operator_browser_requests: true,
          no_storage: true,
          errors,
          paid_calls: 0,
        },
        null,
        2,
      ) + '\n',
    )
  } finally {
    await request.post(`${base}/pause`, { headers: opHeaders })
    await Promise.all(contexts.map((c) => c.close()))
  }
})
