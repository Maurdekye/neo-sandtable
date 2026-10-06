import { expect, test } from '@playwright/test'
import { writeFileSync } from 'node:fs'
test('records real and synthetic navigation timing', async ({
  page,
  browser,
}) => {
  const results = []
  for (const url of ['/', '/?map=synthetic']) {
    await page.goto(url)
    await expect(page.locator('.board canvas')).toBeVisible()
    await expect(page.getByTestId('fps')).not.toHaveText('0 FPS · WebGL')
    // Warm caches before measuring. Each rAF drives the same DOM input handlers as a spectator.
    const bounds = (await page.locator('.board canvas').boundingBox())!
    await page.mouse.move(
      bounds.x + bounds.width / 2,
      bounds.y + bounds.height / 2,
    )
    await page.mouse.down()
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(e.message))
    const result = await page.evaluate(async () => {
      const canvas = document.querySelector(
          '.board canvas',
        ) as HTMLCanvasElement,
        rect = canvas.getBoundingClientRect()
      const x = rect.left + rect.width / 2,
        y = rect.top + rect.height / 2
      await new Promise<void>((resolve) => window.setTimeout(resolve, 1500))
      const gl = canvas.getContext('webgl2') ?? canvas.getContext('webgl')
      const info = gl?.getExtension('WEBGL_debug_renderer_info')
      const renderer =
        gl && info
          ? (gl.getParameter(info.UNMASKED_RENDERER_WEBGL) as string)
          : 'unreported'
      const intervals: number[] = []
      let start = 0,
        last = 0,
        n = 0
      await new Promise<void>((resolve) => {
        function sample(now: number) {
          if (!start) {
            start = last = now
          } else {
            intervals.push(now - last)
            last = now
          }
          n++
          canvas.dispatchEvent(
            new PointerEvent('pointermove', {
              clientX: x + 160 * Math.sin(n / 24),
              clientY: y + 70 * Math.sin(n / 30),
              pointerId: 1,
              bubbles: true,
            }),
          )
          if (n % 30 === 0)
            canvas.dispatchEvent(
              new WheelEvent('wheel', {
                clientX: x,
                clientY: y,
                deltaY: n % 60 === 0 ? 160 : -160,
                bubbles: true,
                cancelable: true,
              }),
            )
          if (now - start >= 8000) {
            canvas.dispatchEvent(
              new PointerEvent('pointerup', {
                clientX: x,
                clientY: y,
                pointerId: 1,
                bubbles: true,
              }),
            )
            resolve()
          } else requestAnimationFrame(sample)
        }
        requestAnimationFrame(sample)
      })
      intervals.sort((a, b) => a - b)
      return {
        hexes: document.querySelector('.map-caption')?.textContent,
        duration_ms: last - start,
        frames: intervals.length,
        fps: (intervals.length * 1000) / (last - start),
        median_ms: intervals[Math.floor(intervals.length / 2)],
        p95_ms: intervals[Math.floor(intervals.length * 0.95)],
        renderer,
        userAgent: navigator.userAgent,
        viewport: [innerWidth, innerHeight],
        dpr: devicePixelRatio,
        active_overlays:
          'none; 18 generated counters, mock stream active, three seat tabs',
      }
    })
    await page.mouse.up()
    expect(errors).toEqual([])
    results.push(result)
  }
  writeFileSync(
    '../../navigation-benchmark.json',
    JSON.stringify({ browser_version: browser.version(), results }, null, 2),
  )
})
