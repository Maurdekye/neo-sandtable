//! The seat tools consume only this async actor adapter, never authoritative state.
use crate::{CampaignStatus, Error, actor::CampaignHandle};
use async_trait::async_trait;
use cna_core::{decision::DecisionRequest, engine::Rejection, ids::SeatId};
use cna_protocol::TranscriptEntry;
use cna_seats::{
    game::{GameBackend, SubmitReceipt, SubmitRequest, ToolError},
    memory::{SeatMemory, TeamMessage, WriteMode},
    transcript::TranscriptStore,
};
use serde_json::{Value, json};

pub fn seat_error(error: Error) -> ToolError {
    match error {
        Error::StaleEpoch => ToolError::EpochMismatch,
        Error::Rejected(Rejection::UnknownDecision { decision_id })
        | Error::Rejected(Rejection::WrongSeat { decision_id, .. }) => {
            ToolError::UnknownDecision(decision_id.to_string())
        }
        Error::Rejected(Rejection::StaleRevision { expected, got }) => {
            ToolError::Stale(format!("decision revision {got}; current is {expected}"))
        }
        Error::Rejected(Rejection::Illegal { message }) => ToolError::Illegal(message),
        Error::IdempotencyConflict => {
            ToolError::Illegal("idempotency key identifies another submission".into())
        }
        Error::Invalid(message) => ToolError::Illegal(message),
        // Storage paths, invariant descriptions and unsupported-case internals are operator-only.
        _ => ToolError::Other("campaign unavailable; retry or ask the operator".into()),
    }
}
#[async_trait]
impl GameBackend for CampaignHandle {
    async fn seats(&self) -> Vec<SeatId> {
        SeatId::all().collect()
    }
    /// This legacy unscoped method cannot safely disclose a global sequence. Use the seat
    /// projection's seq; transcript alignment is performed by the persistent store instead.
    async fn game_seq(&self) -> u64 {
        0
    }
    async fn pending(&self, seat: SeatId) -> Vec<DecisionRequest> {
        self.seat(seat).pending
    }
    async fn observe(&self, seat: SeatId) -> Value {
        self.observation(seat)
    }
    async fn inspect(&self, seat: SeatId, target: &str) -> Result<Value, ToolError> {
        CampaignHandle::inspect(self, seat, target)
            .await
            .map_err(seat_error)
    }
    async fn describe_actions(&self, seat: SeatId, id: &str) -> Result<Value, ToolError> {
        let request = self
            .seat(seat)
            .pending
            .into_iter()
            .find(|d| d.id.as_str() == id)
            .ok_or_else(|| ToolError::UnknownDecision(id.into()))?;
        Ok(json!({"request":request,"action_schema":request.space.to_json_schema()}))
    }
    async fn validate(&self, seat: SeatId, id: &str, action: &Value) -> Result<Value, ToolError> {
        self.validate_action(seat, id, action.clone())
            .await
            .map_err(seat_error)?;
        Ok(json!({"valid":true}))
    }
    async fn submit(
        &self,
        seat: SeatId,
        request: SubmitRequest,
    ) -> Result<SubmitReceipt, ToolError> {
        let id = request.decision_id.clone();
        let receipt = self
            .submit_action(seat, request)
            .await
            .map_err(seat_error)?;
        // Stable original outcome on every replay; observation is obtained separately.
        Ok(SubmitReceipt {
            decision_id: id.clone(),
            duplicate: receipt.duplicate,
            summary: format!("accepted decision {id}"),
            result: json!({"accepted":true}),
        })
    }
    async fn epoch(&self, seat: SeatId) -> u64 {
        self.seat(seat).binding.controller_epoch
    }
    async fn outcome(&self) -> Option<Value> {
        match self.status() {
            CampaignStatus::Finished { summary } => Some(json!({"summary":summary})),
            _ => None,
        }
    }
}
#[async_trait]
impl SeatMemory for CampaignHandle {
    async fn notebook_read(&self, seat: SeatId) -> String {
        self.notebook(seat).await.unwrap_or_default()
    }
    async fn notebook_write(
        &self,
        seat: SeatId,
        mode: WriteMode,
        text: &str,
    ) -> Result<usize, String> {
        self.write_notebook(seat, mode, text)
            .await
            .map_err(|_| "notebook unavailable or exceeds its size limit".into())
    }
    async fn message_team(&self, seat: SeatId, text: &str) -> Result<usize, String> {
        CampaignHandle::message_team(self, seat, text)
            .await
            .map_err(|_| "team message unavailable or exceeds its size limit".into())
    }
    async fn read_messages(&self, seat: SeatId, after: u64) -> Vec<TeamMessage> {
        self.messages(seat, after).await.unwrap_or_default()
    }
}
#[async_trait]
impl TranscriptStore for CampaignHandle {
    async fn append(
        &self,
        seat: SeatId,
        at: String,
        entry: TranscriptEntry,
    ) -> Result<u64, String> {
        self.transcript(seat, at, entry)
            .await
            .map_err(|_| "transcript storage unavailable".into())
    }
}
