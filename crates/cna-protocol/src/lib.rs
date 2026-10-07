//! Wire types for the live stream between the neo-sandtable server and its viewers (the web
//! board) and the seat drivers. This crate is the single source of truth for those shapes:
//! `cargo test -p cna-protocol` regenerates the TypeScript in `web/src/generated/`.
//!
//! See `docs/protocol.md` for the flow (subscribe → hello → snapshot → events, plus the
//! transcript channel) and the visibility rules behind perspectives.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Sequence numbers (`seq`, `tseq`, `game_seq`, …) are `u64` in Rust but travel as JSON numbers
/// and are typed `number` in TypeScript; they stay far below 2^53.
///
/// Bumped whenever a breaking change is made to these types.
pub const PROTOCOL_VERSION: u32 = 1;

/// One of the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Axis,
    Commonwealth,
}

impl Side {
    pub const ALL: [Side; 2] = [Side::Axis, Side::Commonwealth];

    pub fn opponent(self) -> Side {
        match self {
            Side::Axis => Side::Commonwealth,
            Side::Commonwealth => Side::Axis,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Side::Axis => "axis",
            Side::Commonwealth => "commonwealth",
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Side {
    type Err = UnknownName;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Side::ALL
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| UnknownName::new("side", s))
    }
}

/// A command role. Each side has one seat per role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Commander,
    FrontLine,
    RearArea,
    Logistics,
    Air,
}

impl Role {
    pub const ALL: [Role; 5] = [
        Role::Commander,
        Role::FrontLine,
        Role::RearArea,
        Role::Logistics,
        Role::Air,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Commander => "commander",
            Role::FrontLine => "front_line",
            Role::RearArea => "rear_area",
            Role::Logistics => "logistics",
            Role::Air => "air",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = UnknownName;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Role::ALL
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| UnknownName::new("role", s))
    }
}

/// A name that does not match any variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownName {
    pub kind: &'static str,
    pub value: String,
}

impl UnknownName {
    pub fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}

impl std::fmt::Display for UnknownName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown {}: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for UnknownName {}

/// A seat id of the form `<side>.<role>`, e.g. `axis.logistics`.
pub type SeatId = String;

/// Who a stream is filtered for: `operator` (omniscient), `side:<side>`, or `seat:<seat id>`.
/// Kept as a string on the wire; the server parses and validates it.
pub type Perspective = String;

/// Messages a viewer sends over the WebSocket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Start (or resume) a stream. `from_seq: None` asks for a fresh snapshot.
    Subscribe {
        perspective: Perspective,
        #[ts(type = "number | null")]
        from_seq: Option<u64>,
    },
}

/// Messages the server sends over the WebSocket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello {
        protocol: u32,
        campaign: CampaignMeta,
        perspective: Perspective,
    },
    /// The full projected state as of event `seq`.
    Snapshot {
        #[ts(type = "number")]
        seq: u64,
        view: ViewState,
    },
    /// One game event; `seq` increases by exactly one each time. `hex` and `unit_id`, when
    /// present, locate an event that carries no hex of its own (dice, notes); they are only ever
    /// set to what this perspective may know.
    Event {
        #[ts(type = "number")]
        seq: u64,
        clock: Clock,
        event: GameEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        hex: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        unit_id: Option<String>,
    },
    /// A live entry from an AI seat's session. Never affects adjudication.
    Transcript {
        seat: SeatId,
        /// Per-seat transcript sequence, strictly increasing.
        #[ts(type = "number")]
        tseq: u64,
        /// Wall-clock capture time (RFC 3339), for display only.
        at: String,
        /// The latest game event `seq` when this entry was captured, to align replays.
        #[ts(type = "number")]
        game_seq: u64,
        entry: TranscriptEntry,
    },
    /// The client must discard its state and subscribe again.
    Resync,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CampaignMeta {
    pub id: String,
    pub scenario_id: String,
    pub rules_profile: String,
    pub title: String,
    pub seats: Vec<SeatInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SeatInfo {
    pub id: SeatId,
    pub side: Side,
    pub role: Role,
    pub controller: Option<ControllerInfo>,
    pub status: SeatStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ControllerInfo {
    pub kind: ControllerKind,
    /// Human-readable description, e.g. "Claude Code (claude-5) · sonnet".
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum ControllerKind {
    #[serde(rename = "scripted")]
    Scripted,
    #[serde(rename = "llm-cli")]
    LlmCli,
    #[serde(rename = "system1")]
    System1,
    #[serde(rename = "human")]
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SeatStatus {
    Idle,
    Deciding,
    Paused,
    Failed,
}

/// Where the campaign is in the sequence of play. `stage`, `phase`, `segment` and `step` use
/// the timing vocabulary of `data/rules/README.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Clock {
    /// 1..=111; one Game-Turn is one week.
    pub game_turn: u16,
    /// Game date of the turn's first day, ISO 8601 (e.g. "1940-09-15").
    pub date: String,
    pub stage: String,
    pub op_stage: Option<u8>,
    pub phase: String,
    pub segment: Option<String>,
    pub step: Option<String>,
    /// The phasing side, once Player A/B are resolved.
    pub phasing: Option<Side>,
}

/// Everything a perspective may see at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ViewState {
    pub clock: Clock,
    pub stacks: Vec<Stack>,
    /// Units this perspective may know about in detail, by id.
    pub units: BTreeMap<String, UnitView>,
    pub markers: Vec<Marker>,
    pub pending: Vec<PendingDecision>,
}

/// A stack on the map. Under `land:3.6` the presence of an enemy stack is public but its
/// contents are not, so `unit_ids` may be empty and `visible_count` absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Stack {
    /// Printed hex id, e.g. "C4218".
    pub hex: String,
    pub side: Side,
    pub unit_ids: Vec<String>,
    pub visible_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct UnitView {
    pub id: String,
    pub side: Side,
    pub name: String,
    /// e.g. "infantry", "armor", "artillery", "hq", "truck".
    pub kind: String,
    /// NATO echelon: "company", "battalion", "regiment", "brigade", "division", …
    pub size: String,
    /// e.g. "italian", "british", "australian", "indian", "new_zealand".
    pub nationality: String,
    pub hex: Option<String>,
    /// Organizational parent (assigned or attached), for the inspector tree.
    pub parent: Option<String>,
    /// Authorized details (CPA, TOE strength, supplies, …), keyed by field name.
    pub detail: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Marker {
    pub id: String,
    /// e.g. "supply_dump", "fortification", "minefield", "airfield".
    pub kind: String,
    pub hex: String,
    pub side: Option<Side>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PendingDecision {
    pub id: String,
    pub seat: SeatId,
    pub kind: String,
    pub summary: String,
    #[ts(type = "number")]
    pub opened_seq: u64,
    /// Rule citations governing the decision (`land:7.11`); empty when the ruleset gives none.
    #[serde(default)]
    pub rules: Vec<String>,
    /// The legal action space as JSON Schema, for the owning seat, its side and the operator
    /// (pending decisions are filtered by perspective). Absent when the ruleset gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub space: Option<serde_json::Value>,
}

/// Game events as seen by a perspective. Viewers must ignore kinds they do not know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GameEvent {
    /// The sequence of play moved on.
    PhaseChanged {
        clock: Clock,
    },
    /// A unit (or stack member) moved along a path of printed hex ids.
    UnitMoved {
        unit_id: String,
        path: Vec<String>,
        cp_spent: Option<i32>,
    },
    /// A unit's visible state changed (strength, status, parent, …).
    UnitUpdated {
        unit: UnitView,
    },
    /// The stack of `side` in `hex` now looks like this to the perspective (replaces any
    /// previous stack of that side in that hex). An enemy stack typically carries no unit ids.
    StackUpdated {
        stack: Stack,
    },
    /// The stack of `side` in `hex` is gone (moved away, destroyed, or no longer visible).
    StackRemoved {
        hex: String,
        side: Side,
    },
    /// A unit left play (destroyed, surrendered, withdrawn) or left this perspective's view.
    UnitRemoved {
        unit_id: String,
        reason: String,
    },
    /// Dice were rolled; `reading` is the two-dice 11–66 value when the rule reads them so.
    DiceRolled {
        purpose: String,
        dice: Vec<u8>,
        reading: Option<u8>,
        rule: Option<String>,
    },
    /// A combat was resolved in a hex; details are filtered per perspective.
    CombatResolved {
        hex: String,
        summary: String,
        detail: Option<BTreeMap<String, serde_json::Value>>,
    },
    DecisionOpened {
        decision: PendingDecision,
    },
    DecisionResolved {
        decision_id: String,
        seat: SeatId,
        summary: String,
    },
    MarkerPlaced {
        marker: Marker,
    },
    MarkerRemoved {
        marker_id: String,
    },
    /// Free-form, human-readable note from the engine (e.g. a weather result).
    Note {
        text: String,
    },
}

/// One entry of an AI seat's transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptEntry {
    AssistantText {
        text: String,
    },
    /// Only when the CLI exposes it.
    Reasoning {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool: String,
        args: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        summary: String,
        detail: Option<serde_json::Value>,
    },
    DecisionSubmitted {
        decision_id: String,
        summary: String,
    },
    System {
        text: String,
    },
    System1Query {
        question: String,
        options: Vec<String>,
    },
    System1Answer {
        choice: String,
        scores: Option<BTreeMap<String, f64>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Regenerates the TypeScript bindings in `web/src/generated/`.
    #[test]
    fn export_typescript_bindings() {
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/src/generated");
        let cfg = ts_rs::Config::new().with_out_dir(&out);
        ServerMessage::export_all(&cfg).expect("export ServerMessage");
        ClientMessage::export_all(&cfg).expect("export ClientMessage");
    }

    #[test]
    fn messages_use_documented_tags() {
        let msg = ServerMessage::Transcript {
            seat: "axis.commander".into(),
            tseq: 1,
            at: "2026-10-06T10:32:28.693Z".into(),
            game_seq: 0,
            entry: TranscriptEntry::ToolCall {
                call_id: "c1".into(),
                tool: "observe".into(),
                args: serde_json::json!({}),
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "transcript");
        assert_eq!(json["entry"]["kind"], "tool_call");

        let sub: ClientMessage = serde_json::from_str(
            r#"{"type":"subscribe","perspective":"side:axis","from_seq":null}"#,
        )
        .unwrap();
        assert_eq!(
            sub,
            ClientMessage::Subscribe {
                perspective: "side:axis".into(),
                from_seq: None
            }
        );

        let kind = serde_json::to_value(ControllerKind::LlmCli).unwrap();
        assert_eq!(kind, "llm-cli");
    }
}
