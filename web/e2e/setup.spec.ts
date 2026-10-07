import { expect, test } from '@playwright/test'
import { writeFileSync } from 'node:fs'
import type { ViewState } from '../src/protocol'
const server = process.env.CNA_SMOKE_SERVER,
  capability = process.env.CNA_SMOKE_CAPABILITY
const headers = { Authorization: `Bearer ${capability ?? ''}` }
test('watches real blind Graziani setup, authorized choices and scripted reveal', async ({
  page,
  browser,
  request,
}) => {
  test.skip(
    !server || !capability,
    'Set CNA_SMOKE_SERVER and CNA_SMOKE_CAPABILITY',
  )
  test.setTimeout(240000)
  const started = Date.now()
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  const created = await request.post(`${server}/api/campaigns`, {
    headers,
    data: {
      kind: 'cna',
      rules_profile: 'cna-2021-dev',
      seed: Array(32).fill(0),
      title: "Graziani's Offensive - blind set-up",
      paused: true,
      controller: 'human',
    },
  })
  expect(created.ok()).toBeTruthy()
  const meta = await created.json(),
    base = `${server}/api/campaigns/${meta.id}`
  const caps = await (
    await request.get(`${base}/capabilities`, { headers })
  ).json()
  await page.addInitScript(
    ({ server, capability }) =>
      sessionStorage.setItem(`cna:cap:${new URL(server).origin}`, capability),
    { server: server!, capability: capability! },
  )
  await page.goto(`${server}/?campaign=${meta.id}`)
  await expect(page.getByTestId('playback-status')).toContainText('LIVE')
  await page
    .getByLabel('Perspective', { exact: true })
    .selectOption('side:commonwealth')
  expect((await request.post(`${base}/resume`, { headers })).ok()).toBeTruthy()
  await expect(page.locator('.setup-unit').first()).toBeVisible({
    timeout: 15000,
  })
  const projection = await (
      await request.get(`${base}?perspective=side:commonwealth`, {
        headers: { Authorization: `Bearer ${caps.sides.commonwealth}` },
      })
    ).json(),
    view: ViewState = projection.snapshot.view
  const decision = view.pending.find(
    (d) =>
      d.kind === 'cna.setup.unit' &&
      (
        d.space as { enum?: string[]; 'x-context'?: { group?: string } }
      )?.enum?.includes('E1730') &&
      (d.space as { 'x-context'?: { group?: string } })?.['x-context']
        ?.group === 'cw_cairo',
  )!
  expect(decision, 'Cairo placement decision is projected').toBeTruthy()
  const schema = decision.space as {
    enum: string[]
    'x-context': { unit: string; group: string }
  }
  const unitId = schema['x-context'].unit,
    unit = view.units[unitId],
    destination = 'E1730'
  expect(unit).toBeTruthy()
  expect(unit.hex).toBeNull()
  expect(schema.enum).toContain(destination)
  const panel = page.locator(
    `.pending-decisions article[data-decision-id="${decision.id}"]`,
  )
  await expect(panel.locator('.setup-unit')).toContainText(unit.name)
  await panel.getByRole('button', { name: /legal area/ }).click()
  await expect(page.getByTestId('placement-highlight')).toContainText(
    `${schema.enum.length} legal set-up hexes`,
  )
  await panel.locator('.setup-unit').click()
  await expect(page.locator('.unit-detail')).toContainText('Awaiting setup')
  await panel.locator('.setup-destinations summary').click()
  await panel.scrollIntoViewIfNeeded()
  await page.screenshot({
    path: '../../board-graziani-setup.png',
    fullPage: true,
  })
  const enemyContext = await browser.newContext()
  try {
    await enemyContext.addInitScript(
      ({ server, token }) =>
        sessionStorage.setItem(`cna:cap:${new URL(server).origin}`, token),
      { server: server!, token: caps.sides.axis },
    )
    const enemy = await enemyContext.newPage()
    enemy.on('pageerror', (e) => errors.push(e.message))
    const enemyUnitFrames: string[] = []
    enemy.on('websocket', (socket) =>
      socket.on('framereceived', (frame) => {
        const value =
          typeof frame.payload === 'string'
            ? frame.payload
            : frame.payload.toString()
        if (value.includes(unitId)) enemyUnitFrames.push(value)
      }),
    )
    await enemy.goto(`${server}/?campaign=${meta.id}`)
    await expect(enemy.getByLabel('Perspective', { exact: true })).toHaveValue(
      'side:axis',
    )
    await expect(enemy.getByTestId('playback-status')).toContainText('LIVE')
    await expect(
      enemy.locator(`.setup-unit[data-unit-id="${unitId}"]`),
    ).toHaveCount(0)
    const enemyHeaders = { Authorization: `Bearer ${caps.sides.axis}` }
    const before = await (
      await request.get(`${base}?perspective=side:axis`, {
        headers: enemyHeaders,
      })
    ).json()
    expect(before.snapshot.view.units[unitId]).toBeUndefined()
    const observation = await (
      await request.get(`${base}/seats/${decision.seat}/observe`, { headers })
    ).json()
    const core = observation.pending.find(
      (d: { id: string }) => d.id === decision.id,
    )
    expect(
      (
        await request.post(
          `${base}/seats/${decision.seat}/decisions/${decision.id}/validate`,
          { headers, data: { action: destination } },
        )
      ).ok(),
    ).toBeTruthy()
    const submitted = await request.post(
      `${base}/seats/${decision.seat}/decisions/${decision.id}/submit`,
      {
        headers,
        data: {
          decision_id: decision.id,
          seat: decision.seat,
          controller_epoch: observation.controller_epoch,
          decision_revision: core.revision,
          idempotency_key: 'board-setup-proof',
          action: destination,
          public_explanation: null,
        },
      },
    )
    expect(submitted.ok()).toBeTruthy()
    await expect(panel).toHaveCount(0)
    const after = await (
      await request.get(`${base}?perspective=side:axis`, {
        headers: enemyHeaders,
      })
    ).json()
    expect(after.snapshot.view.stacks).toEqual(before.snapshot.view.stacks)
    expect(after.snapshot.view.units[unitId]).toBeUndefined()
    const ownBuffered = await (
      await request.get(`${base}?perspective=side:commonwealth`, {
        headers: { Authorization: `Bearer ${caps.sides.commonwealth}` },
      })
    ).json()
    expect(ownBuffered.snapshot.view.units[unitId].hex).toBeNull()
    const barrier = 'axis.commander'
    for (const seat of meta.seats.filter(
      (s: { id: string }) => s.id !== barrier,
    )) {
      expect(
        (
          await request.post(`${base}/seats/${seat.id}/controller`, {
            headers,
            data: {
              controller: seat.id.endsWith('.air')
                ? { kind: 'human', label: 'Enumerated air test fixture' }
                : { kind: 'scripted', label: 'pass_when_possible' },
              config: { mode: 'pass_when_possible' },
            },
          })
        ).ok(),
      ).toBeTruthy()
    }
    // Air has no board projection. Complete its enumerated choices with a bounded fixture,
    // keeping the actual map placements driven by server-scripted seats.
    for (let count = 0; count < 200; count++) {
      const current = await (await request.get(base, { headers })).json()
      const air = current.snapshot.view.pending.find(
        (d: { kind: string }) => d.kind === 'cna.setup.air',
      )
      if (!air) break
      expect(count, 'bounded initial air allocation').toBeLessThan(199)
      const observed = await (
        await request.get(`${base}/seats/${air.seat}/observe`, { headers })
      ).json()
      const core = observed.pending.find((d: { id: string }) => d.id === air.id)
      const action = air.space.enum[0]
      expect(typeof action).toBe('string')
      expect(
        (
          await request.post(
            `${base}/seats/${air.seat}/decisions/${air.id}/submit`,
            {
              headers,
              data: {
                decision_id: air.id,
                seat: air.seat,
                controller_epoch: observed.controller_epoch,
                decision_revision: core.revision,
                idempotency_key: `board-air-fixture:${air.id}`,
                action,
                public_explanation: null,
              },
            },
          )
        ).ok(),
      ).toBeTruthy()
    }
    await expect
      .poll(
        async () => {
          const v = await (await request.get(base, { headers })).json()
          return v.snapshot.view.pending.filter(
            (d: { seat: string; kind: string }) =>
              d.kind.startsWith('cna.setup.') && d.seat !== barrier,
          ).length
        },
        { timeout: 180000, intervals: [1000, 2000] },
      )
      .toBe(0)
    const stillBlind = await (
      await request.get(`${base}?perspective=side:axis`, {
        headers: enemyHeaders,
      })
    ).json()
    expect(stillBlind.snapshot.view.stacks).toEqual(before.snapshot.view.stacks)
    // Stop before movement/combat so their later unit updates cannot mask a missing setup reveal.
    for (const side of ['axis', 'commonwealth']) {
      expect(
        (
          await request.post(`${base}/seats/${side}.front_line/controller`, {
            headers,
            data: {
              controller: { kind: 'human', label: 'Hold after setup' },
              config: {},
            },
          })
        ).ok(),
      ).toBeTruthy()
    }
    // Completing the held placement may open optional preload windows before publication.
    const animation = page.waitForFunction(
      () =>
        Number(
          document
            .querySelector('[data-testid="motion-count"]')
            ?.textContent?.split(' ')[0],
        ) > 0,
      {},
      { timeout: 90000 },
    )
    expect(
      (
        await request.post(`${base}/seats/${barrier}/controller`, {
          headers,
          data: {
            controller: { kind: 'scripted', label: 'pass_when_possible' },
            config: { mode: 'pass_when_possible' },
          },
        })
      ).ok(),
    ).toBeTruthy()
    await animation
    await expect(page.getByTestId('placement-highlight')).toHaveCount(0, {
      timeout: 15000,
    })
    expect((await request.post(`${base}/pause`, { headers })).ok()).toBeTruthy()
    const ownPlaced = await (
      await request.get(`${base}?perspective=side:commonwealth`, {
        headers: { Authorization: `Bearer ${caps.sides.commonwealth}` },
      })
    ).json()
    expect(ownPlaced.snapshot.view.units[unitId].hex).toBe(destination)
    expect(ownPlaced.snapshot.view.clock.segment).not.toBe('combat')
    const opponentPlaced = await (
      await request.get(`${base}?perspective=side:axis`, {
        headers: enemyHeaders,
      })
    ).json()
    const revealed = opponentPlaced.snapshot.view.stacks.find(
      (s: { hex: string; side: string }) =>
        s.hex === destination && s.side === 'commonwealth',
    )
    expect(revealed).toMatchObject({ unit_ids: [], visible_count: null })
    expect(opponentPlaced.snapshot.view.units[unitId]).toBeUndefined()
    expect(enemyUnitFrames).toEqual([])
    await page.getByLabel('Find formation or unit').fill(unitId)
    await page.locator(`.formation-unit[data-unit-id="${unitId}"]`).click()
    await expect(page.locator('.unit-detail')).toContainText(destination)
    await page.getByRole('tab', { name: /commonwealth.*commander/ }).click()
    await expect(
      page.locator('.entry-decision_submitted').first(),
    ).toBeVisible()
    await page.screenshot({
      path: '../../board-graziani-setup-revealed.png',
      fullPage: true,
    })
    expect(errors).toEqual([])
    writeFileSync(
      '../../setup-browser-verification.json',
      JSON.stringify(
        {
          commit: process.env.CNA_SMOKE_COMMIT,
          elapsed_ms: Date.now() - started,
          reveal_before_combat: true,
          unit: unitId,
          name: unit.name,
          destination,
          legal_destinations: schema.enum,
          blind_enemy_unchanged: true,
          scripted_animation: true,
          unit_identity_hidden_after_reveal: true,
          errors,
        },
        null,
        2,
      ),
    )
  } finally {
    await enemyContext.close()
    await request.post(`${base}/pause`, { headers })
  }
})
