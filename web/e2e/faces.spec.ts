import { expect, test, type Page } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import type { ServerMessage, UnitView, ViewState } from '../src/protocol'
const server = process.env.CNA_SMOKE_SERVER,
  operator = process.env.CNA_SMOKE_CAPABILITY
const cells = readFileSync('../data/map/hexes.csv', 'utf8')
  .trim()
  .split(/\r?\n/)
  .slice(1)
  .map((row) => {
    const fields = row.split(',')
    return { id: fields[0], q: Number(fields[4]), r: Number(fields[5]) }
  })
const cell = new Map(cells.map((c) => [c.id, c]))
const center = (h: { q: number; r: number }) => ({
  x: 26 * Math.sqrt(3) * (h.q + h.r / 2),
  y: 39 * h.r,
})
function assertFace(unit: UnitView) {
  expect(unit.hex !== null).toBe(true)
  expect(unit.parent).toBeNull()
  expect(
    Object.keys(unit.detail ?? {}).every((k) =>
      ['counter', 'stacking_points'].includes(k),
    ),
  ).toBe(true)
}
function watch(page: Page, snapshots: ViewState[], errors: string[]) {
  page.on('pageerror', (e) =>
    errors.push(e.message.replace(/[a-f0-9]{64}/g, '[redacted]')),
  )
  page.on('websocket', (ws) =>
    ws.on('framereceived', (f) => {
      const m: ServerMessage = JSON.parse(String(f.payload))
      if (m.type === 'snapshot') snapshots.push(m.view)
      if (
        m.type === 'event' &&
        m.event.kind === 'unit_updated' &&
        m.event.unit.side === 'commonwealth'
      )
        assertFace(m.event.unit)
    }),
  )
}
test('production board and isolated console receive and inspect real printed enemy faces', async ({
  browser,
  request,
}) => {
  test.skip(
    !server || !operator,
    'Requires a fresh own authenticated no-paid server',
  )
  const headers = { Authorization: `Bearer ${operator}` }
  const created = await request.post(`${server}/api/campaigns`, {
    headers,
    data: {
      kind: 'cna',
      rules_profile: 'cna-2021-dev',
      seed: Array(32).fill(0),
      title: 'Printed faces receive-only proof',
      paused: true,
      controller: 'legal_random',
    },
  })
  expect(created.ok()).toBe(true)
  const meta = await created.json(),
    base = `${server}/api/campaigns/${meta.id}`,
    seat = 'axis.commander'
  expect(
    (
      await request.post(`${base}/seats/${seat}/controller`, {
        headers,
        data: {
          controller: { kind: 'human', label: 'Receive-only proof' },
          config: {},
        },
      })
    ).ok(),
  ).toBe(true)
  const caps = await (
    await request.get(`${base}/capabilities`, { headers })
  ).json()
  const full = await (await request.get(base, { headers })).json()
  const operatorView: ViewState = full.snapshot.view
  const contexts = await Promise.all([
    browser.newContext(),
    browser.newContext(),
  ])
  const [board, consolePage] = await Promise.all(
    contexts.map((c) => c.newPage()),
  )
  const snapshots: ViewState[][] = [[], []],
    errors: string[] = [],
    calls: { path: string; method: string }[] = []
  try {
    watch(board, snapshots[0], errors)
    watch(consolePage, snapshots[1], errors)
    await consolePage.addInitScript(() => {
      for (const method of ['getItem', 'setItem'])
        Object.defineProperty(Storage.prototype, method, {
          value: () => {
            throw new Error('Console accessed storage')
          },
        })
    })
    consolePage.on('request', (r) => {
      const u = new URL(r.url())
      if (!u.pathname.startsWith('/api/')) return
      calls.push({ path: u.pathname, method: r.method() })
      expect(r.headers().authorization === `Bearer ${caps.seats[seat]}`).toBe(
        true,
      )
      expect(
        u.pathname === '/api/session' ||
          u.pathname.startsWith(`${new URL(base).pathname}/seats/${seat}/`),
      ).toBe(true)
      expect(r.method()).toBe('GET')
    })
    await board
      .goto(`${server}/?campaign=${meta.id}#cap=${caps.sides.axis}`)
      .catch(() => {
        throw new Error('Board navigation failed')
      })
    await consolePage
      .goto(
        `${server}/console.html?campaign=${meta.id}&seat=${seat}#cap=${caps.seats[seat]}`,
      )
      .catch(() => {
        throw new Error('Console navigation failed')
      })
    await expect
      .poll(() => snapshots.map((s) => s.length).every((n) => n > 0))
      .toBe(true)
    await expect(consolePage.getByRole('heading', { level: 1 })).toHaveText(
      seat,
    )
    expect(consolePage.url().includes('#cap=')).toBe(false)
    for (const list of snapshots) {
      const faces = Object.values(list[0].units).filter(
        (u) => u.side === 'commonwealth',
      )
      expect(faces.length).toBeGreaterThan(0)
      faces.forEach(assertFace)
      for (const u of Object.values(operatorView.units).filter(
        (u) => u.side === 'commonwealth',
      )) {
        const parent = u.parent ? operatorView.units[u.parent] : null
        if (!u.hex || (parent && parent.hex === u.hex))
          expect(list[0].units[u.id]).toBeUndefined()
      }
      for (const stack of list[0].stacks.filter(
        (s) => s.side === 'commonwealth',
      )) {
        expect(stack.visible_count).toBe(stack.unit_ids.length)
        expect(
          stack.unit_ids.every(
            (id) => list[0].units[id]?.side === 'commonwealth',
          ),
        ).toBe(true)
      }
    }
    const initial = snapshots[1][0].stacks.find(
      (s) => s.side === 'axis' && cell.has(s.hex),
    )!
    const origin = center(cell.get(initial.hex)!)
    const target = Object.values(snapshots[1][0].units)
      .filter(
        (u) =>
          u.side === 'commonwealth' &&
          typeof u.detail?.counter === 'string' &&
          typeof u.detail?.stacking_points === 'number' &&
          cell.has(u.hex!),
      )
      .sort((a, b) => {
        const distance = (u: UnitView) => {
          const p = center(cell.get(u.hex!)!)
          return Math.hypot(p.x - origin.x, p.y - origin.y)
        }
        return distance(a) - distance(b)
      })[0]
    expect(!!target).toBe(true)
    await board.getByLabel('Find formation or unit').fill(target.id)
    await board.locator(`.formation-unit[data-unit-id="${target.id}"]`).click()
    await expect(
      board.locator('.unit-detail').getByTestId('printed-face'),
    ).toContainText(String(target.detail!.counter))
    await expect(
      board.locator('.unit-detail').getByTestId('printed-face'),
    ).toContainText(`${target.detail!.stacking_points} stacking points`)
    await expect(
      board.locator('.unit-detail').getByTestId('unit-status'),
    ).toHaveCount(0)
    await expect(board.getByTestId('fps')).toContainText(/^[1-9]\d* FPS/)
    const boardFps = await board.getByTestId('fps').innerText()
    await board.screenshot({
      path: '../../board-enemy-printed-faces.png',
      fullPage: true,
    })
    await expect(consolePage.locator('canvas')).toBeVisible()
    await expect(consolePage.getByTestId('fps')).toContainText(/^[1-9]\d* FPS/)
    const box = (await consolePage.locator('canvas').boundingBox())!
    const destination = center(cell.get(target.hex!)!)
    const dx = -(destination.x - origin.x) * 1.25,
      dy = -(destination.y - origin.y) * 1.25
    const steps = Math.ceil(
      Math.max(
        Math.abs(dx) / Math.min(250, box.width / 3),
        Math.abs(dy) / Math.min(250, box.height / 3),
      ),
    )
    for (let i = 0; i < steps; i++) {
      await consolePage.mouse.move(
        box.x + box.width / 2,
        box.y + box.height / 2,
      )
      await consolePage.mouse.down()
      await consolePage.mouse.move(
        box.x + box.width / 2 + dx / steps,
        box.y + box.height / 2 + dy / steps,
        { steps: 5 },
      )
      await consolePage.mouse.up()
    }
    await consolePage.mouse.click(box.x + box.width / 2, box.y + box.height / 2)
    const button = consolePage
      .locator('.console-units button')
      .filter({ hasText: target.name })
    await expect(button.getByTestId('printed-face')).toContainText(
      String(target.detail!.counter),
    )
    await button.click()
    await consolePage
      .getByText('Seat inspection and previews', { exact: true })
      .click()
    await expect(consolePage.locator('.console-map pre')).toContainText(
      target.id,
    )
    const inspected = JSON.parse(
      await consolePage.locator('.console-map pre').innerText(),
    )
    assertFace(inspected.unit)
    expect(inspected.unit.detail).toEqual(target.detail)
    const consoleFps = await consolePage.getByTestId('fps').innerText()
    await consolePage.screenshot({
      path: '../../console-enemy-printed-faces.png',
      fullPage: true,
    })
    expect(errors).toEqual([])
    writeFileSync(
      '../../printed-faces-browser-verification.json',
      JSON.stringify(
        {
          commit: process.env.CNA_SMOKE_COMMIT,
          campaign: meta.id,
          paid_calls: 0,
          receive_only: true,
          enemy_faces: snapshots.map(
            (s) =>
              Object.values(s[0].units).filter((u) => u.side === 'commonwealth')
                .length,
          ),
          target: { id: target.id, hex: target.hex, detail: target.detail },
          operator_or_other_seat_requests: 0,
          console_requests: calls,
          fps: { board: boardFps, console: consoleFps },
          errors,
          limitation:
            'Paused real snapshot and own-seat inspect proof; live public path and sync replacement are covered by unit regressions.',
        },
        null,
        2,
      ),
    )
  } finally {
    await Promise.all(contexts.map((c) => c.close()))
  }
})
