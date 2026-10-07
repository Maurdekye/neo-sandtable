//! Events emitted by accepted transitions.
//!
//! A ruleset emits each fact once per audience that may learn it, already shaped for that
//! audience (e.g. full combat detail to the operator, only the totals to the opponent). The
//! server then filters by [`Perspective`](crate::visibility::Perspective) and numbers the
//! surviving events per perspective.

use serde::{Deserialize, Serialize};

use crate::visibility::{Audience, Perspective};

pub use cna_protocol::GameEvent;

/// One event addressed to one audience.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineEvent {
    pub audience: Audience,
    pub event: GameEvent,
    /// The printed hex the event is about, so viewers can locate dice, notes and other events
    /// that carry no hex of their own. Set it only when the audience may know that hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// The unit the event is about, under the same rule: only one the audience may identify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_id: Option<String>,
}

impl EngineEvent {
    pub fn new(audience: Audience, event: GameEvent) -> Self {
        Self {
            audience,
            event,
            hex: None,
            unit_id: None,
        }
    }

    /// Locate the event at `hex` (see [`EngineEvent::hex`]).
    pub fn at(mut self, hex: impl ToString) -> Self {
        self.hex = Some(hex.to_string());
        self
    }

    /// Name the unit the event is about (see [`EngineEvent::unit_id`]).
    pub fn about(mut self, unit_id: impl ToString) -> Self {
        self.unit_id = Some(unit_id.to_string());
        self
    }

    pub fn public(event: GameEvent) -> Self {
        Self::new(Audience::Public, event)
    }

    pub fn visible_to(&self, perspective: Perspective) -> bool {
        perspective.can_see(&self.audience)
    }
}
