//! Explicit fresh NumberDuel adapter. It cannot wrap a server/GameBackend/recovered campaign.
//! NumberDuel has no forced-pass domain; its forced adapter is closed. Source/inert tests of
//! this adapter never establish Server admission, native confinement or provider dispatch.
use crate::game::{RulesetCore, RulesetGame, SubmitRequest};
use crate::host::*;
use crate::operation::{Allowance, Operation};
use crate::toy::NumberDuel;
use std::sync::Arc;
use tokio::sync::watch;

pub(crate) struct ToyHost {
    game: Arc<RulesetGame<NumberDuel>>,
    snapshot: watch::Sender<HostSnapshot>,
}

fn snapshot(core: &RulesetCore<NumberDuel>) -> HostSnapshot {
    let seats = core.seats();
    HostSnapshot {
        generation: core.game_seq(),
        activated: core.outcome().is_none(),
        status: if core.outcome().is_some() {
            HostStatus::Finished
        } else {
            HostStatus::Running
        },
        epochs: seats.iter().map(|s| (*s, core.epoch(*s))).collect(),
        pending: seats
            .iter()
            .map(|s| {
                (
                    *s,
                    core.pending(*s)
                        .iter()
                        .map(|p| (p.id.to_string(), p.revision))
                        .collect(),
                )
            })
            .collect(),
    }
}
fn check(core: &RulesetCore<NumberDuel>, identity: &HostIdentity) -> Result<(), HostRefusal> {
    if core.outcome().is_some() {
        return Err(HostRefusal::StoppedBeforeWork);
    }
    if core.epoch(identity.seat) != identity.epoch
        || !core
            .pending(identity.seat)
            .iter()
            .any(|p| p.id.as_str() == identity.decision_id && p.revision == identity.revision)
    {
        return Err(HostRefusal::IdentityChanged);
    }
    Ok(())
}
impl ToyHost {
    pub(crate) fn fresh_number_duel(game: Arc<RulesetGame<NumberDuel>>) -> Arc<Self> {
        let initial = game.with_core(|core| snapshot(core));
        let (snapshot, _) = watch::channel(initial);
        Arc::new(Self { game, snapshot })
    }
}
struct ToyStep {
    game: Arc<RulesetGame<NumberDuel>>,
    identity: HostIdentity,
}
impl HostStepLease for ToyStep {
    fn perform_sync(
        &mut self,
        effect: &mut dyn FnMut() -> Result<(), HostRefusal>,
    ) -> Result<(), HostRefusal> {
        self.game.with_core(|core| {
            check(core, &self.identity)?;
            effect()
        })
    }
}
struct ToyPrompt {
    game: Arc<RulesetGame<NumberDuel>>,
    identity: HostIdentity,
    allowance: Allowance,
}
#[async_trait::async_trait]
impl HostPromptLease for ToyPrompt {
    fn check(&self) -> Result<(), HostRefusal> {
        self.allowance.check().map_err(operation_refusal)?;
        self.game.with_core(|core| check(core, &self.identity))
    }
    async fn settle(&self) -> Result<(), HostRefusal> {
        // This typed toy has no writer/prompt actor. Native actual supervision is still required
        // by the driver before calling this method; it grants no Server settlement capability.
        Ok(())
    }
}
impl HostAdmission for ToyHost {
    fn subscribe(&self) -> watch::Receiver<HostSnapshot> {
        self.snapshot.subscribe()
    }
    fn snapshot(&self) -> Result<HostSnapshot, HostRefusal> {
        let snapshot = self.game.with_core(|core| snapshot(core));
        let changed = *self.snapshot.borrow() != snapshot;
        if changed {
            self.snapshot.send_replace(snapshot.clone());
        }
        Ok(snapshot)
    }
    fn acquire(
        &self,
        identity: &HostIdentity,
        _: HostStep,
    ) -> Result<Box<dyn HostStepLease>, HostRefusal> {
        self.game.with_core(|core| check(core, identity))?;
        Ok(Box::new(ToyStep {
            game: self.game.clone(),
            identity: identity.clone(),
        }))
    }
    fn acquire_prompt(
        &self,
        identity: &HostIdentity,
        allowance: Allowance,
    ) -> Result<Box<dyn HostPromptLease>, HostRefusal> {
        allowance.check().map_err(operation_refusal)?;
        self.game.with_core(|core| check(core, identity))?;
        Ok(Box::new(ToyPrompt {
            game: self.game.clone(),
            identity: identity.clone(),
            allowance,
        }))
    }
}
impl HostForcedSubmission for ToyHost {
    fn reserve(
        &self,
        _: HostIdentity,
        _: SubmitRequest,
        _: Operation,
    ) -> Result<Box<dyn HostForcedReservation>, HostRefusal> {
        Err(HostRefusal::StoppedBeforeWork)
    }
}
