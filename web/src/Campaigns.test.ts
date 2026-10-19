import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Children, type ReactElement, type ReactNode } from 'react'
import { CampaignControl } from './Campaigns'
import { apiRequest, type Access } from './access'

// Exercise the component's effects, returned controls and handlers in the
// existing Node test environment. This does not claim browser/DOM coverage.
const hooks = vi.hoisted(() => ({
  values: [] as unknown[],
  cursor: 0,
  mounted: false,
  effects: [] as (() => void | (() => void))[],
}))
vi.mock('react', async (original) => ({
  ...(await original<typeof import('react')>()),
  useState: (initial: unknown) => {
    const index = hooks.cursor++
    if (!(index in hooks.values)) hooks.values[index] = initial
    return [
      hooks.values[index],
      (value: unknown) => {
        hooks.values[index] = value
      },
    ]
  },
  useEffect: (effect: () => void | (() => void)) => {
    if (!hooks.mounted) hooks.effects.push(effect)
  },
}))
vi.mock('./access', () => ({ apiRequest: vi.fn() }))

type Props = Parameters<typeof CampaignControl>[0]
type Control = ReactElement<{
  children: ReactNode
  disabled?: boolean
  onClick?: () => void
}>
const request = vi.mocked(apiRequest)
const access: Access = {
  server: 'http://127.0.0.1:3000',
  token: 'synthetic-test-capability',
  session: { operator: true, campaign_id: null, perspective: 'operator' },
}
let cleanups: (() => void)[]

beforeEach(() => {
  vi.useFakeTimers()
  request.mockReset()
  hooks.values = []
  hooks.cursor = 0
  hooks.mounted = false
  hooks.effects = []
  cleanups = []
})
afterEach(() => {
  cleanups.forEach((cleanup) => cleanup())
  vi.useRealTimers()
})

async function flush() {
  // Only mocked request promises and the component's async handler settle.
  for (let i = 0; i < 6; i++) await Promise.resolve()
}
function render(props: Props) {
  hooks.cursor = 0
  const tree = CampaignControl(props)
  const controls = Children.toArray(tree.props.children) as Control[]
  return { status: controls[0], button: controls[1] }
}
function text(node: ReactNode): string {
  return Children.toArray(node).join('')
}
async function mount(state: string, overrides: Partial<Props> = {}) {
  const props: Props = {
    access,
    campaign: 'fixture/one',
    perspective: 'operator',
    history: false,
    ...overrides,
  }
  request.mockResolvedValue({ status: { state, message: 'untrusted message' } })
  render(props)
  hooks.mounted = true
  for (const effect of hooks.effects) {
    const cleanup = effect()
    if (cleanup) cleanups.push(cleanup)
  }
  await flush()
  return () => render(props)
}

describe('campaign recovery activation control', () => {
  it('uses the fixed label and requires a click before Resume; applies the acknowledged state', async () => {
    const view = await mount('recovered_awaiting_activation')
    expect(text(view().status.props.children)).toBe(
      'Campaign recovered, awaiting activation',
    )
    expect(view().button.props.disabled).toBe(false)
    expect(text(view().button.props.children)).toBe('Resume campaign')
    expect(request).toHaveBeenCalledTimes(1)
    expect(request.mock.calls[0][1]).toBe(
      '/api/campaigns/fixture%2Fone?perspective=operator',
    )
    expect(request.mock.calls[0][2]?.method).toBeUndefined()
    request.mockResolvedValueOnce({ paused: false })
    view().button.props.onClick!()
    expect(request.mock.calls[1].slice(1)).toEqual([
      '/api/campaigns/fixture%2Fone/resume',
      { method: 'POST' },
    ])
    await flush()
    expect(text(view().status.props.children)).toBe('Campaign running')
    expect(text(view().button.props.children)).toBe('Pause campaign')
  })

  it.each([
    ['paused', 'Resume campaign', 'resume', false, 'running'],
    ['running', 'Pause campaign', 'pause', true, 'paused'],
  ] as const)(
    'retains the %s action and boolean acknowledgement',
    async (state, label, endpoint, paused, finalState) => {
      const view = await mount(state)
      expect(text(view().button.props.children)).toBe(label)
      request.mockResolvedValueOnce({ paused })
      view().button.props.onClick!()
      await flush()
      expect(request.mock.calls[1][1]).toBe(
        `/api/campaigns/fixture%2Fone/${endpoint}`,
      )
      expect(text(view().status.props.children)).toBe(`Campaign ${finalState}`)
    },
  )

  it.each([
    ['history', { history: true }],
    ['side perspective', { perspective: 'side:axis' as const }],
    ['seat perspective', { perspective: 'seat:axis.commander' as const }],
    [
      'nonoperator session',
      {
        access: {
          ...access,
          session: {
            operator: false,
            campaign_id: 'fixture/one',
            perspective: 'side:axis' as const,
          },
        },
      },
    ],
  ] as const)(
    'retains the %s guard even if its disabled handler is invoked',
    async (_guard, props) => {
      const view = await mount('recovered_awaiting_activation', props)
      expect(view().button.props.disabled).toBe(true)
      view().button.props.onClick!()
      await flush()
      expect(request).toHaveBeenCalledTimes(1)
    },
  )

  it.each([
    'loading',
    'stopped',
    'future_state',
    'recovered_awaiting_activation_extra',
  ])('keeps unknown/nonactionable %s disabled', async (state) => {
    const view = await mount(state)
    expect(view().button.props.disabled).toBe(true)
    view().button.props.onClick!()
    await flush()
    expect(request).toHaveBeenCalledTimes(1)
    expect(text(view().status.props.children)).toBe(`Campaign ${state}`)
  })

  it('disables control while an explicit Resume acknowledgement is pending', async () => {
    const view = await mount('recovered_awaiting_activation')
    let resolve!: (value: unknown) => void
    request.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      }),
    )
    view().button.props.onClick!()
    expect(view().button.props.disabled).toBe(true)
    expect(text(view().button.props.children)).toBe('Applying control…')
    view().button.props.onClick!()
    expect(request).toHaveBeenCalledTimes(2)
    resolve({ paused: false })
    await flush()
    expect(view().button.props.disabled).toBe(false)
  })

  it('does not treat a malformed or rejected acknowledgement as activation', async () => {
    const view = await mount('recovered_awaiting_activation')
    request.mockResolvedValueOnce({ paused: 'false' })
    view().button.props.onClick!()
    await flush()
    expect(text(view().status.props.children)).toContain(
      'recovered, awaiting activation',
    )
    expect(text(view().status.props.children)).toContain(
      'Invalid campaign control acknowledgement',
    )
    expect(text(view().button.props.children)).toBe('Resume campaign')
    request.mockRejectedValueOnce(new Error('Request refused'))
    view().button.props.onClick!()
    await flush()
    expect(text(view().status.props.children)).toContain('Request refused')
    expect(text(view().status.props.children)).not.toContain('Campaign running')
    expect(view().button.props.disabled).toBe(false)
  })

  it('retains five-second polling without automatically activating a recovered campaign', async () => {
    const view = await mount('recovered_awaiting_activation')
    await vi.advanceTimersByTimeAsync(4999)
    expect(request).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(request).toHaveBeenCalledTimes(2)
    expect(
      request.mock.calls.every((call) => call[2]?.method !== 'POST'),
    ).toBe(true)
    expect(text(view().button.props.children)).toBe('Resume campaign')
    cleanups.forEach((cleanup) => cleanup())
    await vi.advanceTimersByTimeAsync(5000)
    expect(request).toHaveBeenCalledTimes(2)
  })
})
