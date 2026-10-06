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
}

impl EngineEvent {
    pub fn new(audience: Audience, event: GameEvent) -> Self {
        Self { audience, event }
    }

    pub fn public(event: GameEvent) -> Self {
        Self::new(Audience::Public, event)
    }

    pub fn visible_to(&self, perspective: Perspective) -> bool {
        perspective.can_see(&self.audience)
    }
}
