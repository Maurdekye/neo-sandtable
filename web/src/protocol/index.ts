/** UI-only aliases. Authoritative protocol structures are generated from cna-protocol. */
export type { CampaignMeta } from '../generated/CampaignMeta'
export type { Clock } from '../generated/Clock'
export type { GameEvent } from '../generated/GameEvent'
export type { SeatInfo } from '../generated/SeatInfo'
export type { Side } from '../generated/Side'
export type { TranscriptEntry } from '../generated/TranscriptEntry'
export type { UnitView } from '../generated/UnitView'
export type { ViewState } from '../generated/ViewState'
export type { ServerMessage } from '../generated/ServerMessage'
import type { ServerMessage } from '../generated/ServerMessage'
import type { ClientMessage } from '../generated/ClientMessage'
export type TranscriptMessage = Extract<ServerMessage, { type: 'transcript' }>
export type Subscribe = ClientMessage
export type Perspective =
  'operator' | 'side:axis' | 'side:commonwealth' | `seat:${string}`
