//! PRIVATE staged driver for the same original creator task; no public caller.
//! One task/handle already stored before either startup or custody gate opens.
//! Host mint/pair stays in original main; this worker never owns a Host/lease.
use super::*;

impl<const N: usize, const R: usize> CreationOwner<N, R> {
    /// Only guarded Fresh / reserved GuardedRecovery. Legacy Recover stays in
    /// its original input slot; this driver never calls a legacy fallback.
    pub(crate) fn dispatch_guarded_pending(&mut self) -> Result<(), IngressRefusal> {
        self.dispatch_guarded_indices(None)
    }

    /// A pre-Ready initializer can start ONLY its retained original attempt.
    fn dispatch_guarded_selected(&mut self, key: CreatorKey) -> Result<usize, IngressRefusal> {
        let index = self.owned_index(key)?;
        if self.started[index] {
            if !self.guarded_started[index] { return Err(IngressRefusal::MissingAttempt); }
        } else {
            let admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
            if admission.closed { return Err(IngressRefusal::Closed); }
            if !matches!(admission.inputs[index].as_ref(),
                Some(CreationInput::Fresh(_)) | Some(CreationInput::GuardedRecovery))
            { return Err(IngressRefusal::MissingAttempt); }
        }
        self.dispatch_guarded_indices(Some(index))?;
        Ok(index)
    }

    fn dispatch_guarded_indices(&mut self, selected: Option<usize>) -> Result<(), IngressRefusal> {
        let used = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?.used;
        for index in 0..used {
            if selected.is_some_and(|only| only != index) { continue; }
            if self.started[index] || self.terminal_published[index] { continue; }
            let input = {
                let mut admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
                if admission.closed { None } else {
                    match admission.inputs[index].as_ref() {
                        Some(CreationInput::Fresh(_)) | Some(CreationInput::GuardedRecovery) => admission.inputs[index].take(),
                        _ => continue,
                    }
                }
            };
            let Some(input) = input else { self.publish_status(index, NOT_STARTED); continue; };
            self.writers[index].record().require_guarded_connection();
            let shared = Arc::clone(&self.shared);
            let directory = Arc::clone(&self.directory);
            let data = Arc::clone(&self.data);
            let writer = self.writers[index].creator_publication();
            let vacant = self.tasks[index].vacant_blocking().expect("one original creator slot");
            vacant.dispatch(&self.runtime, move || {
                let reply = &shared.replies[index];
                if !reply.gate.wait() { return CreatorExit::CancelledBeforeEffects; }
                reply.product.get_or_init(|| {
                    let claimed = writer.claim().expect("one original writer claim");
                    #[cfg(test)] if let Some(gate) = reply.test_preparation_hold.get() {
                        shared.owner_changed.notify_one();
                        reply.test_preparation_reached.get_or_init(|| ());
                        if !gate.wait() { return Err(Error::NotRunning.into()); }
                    }
                    let prepared = match input {
                        CreationInput::Fresh(request) => Some(crate::campaigns::prepare_guarded_fresh(
                            directory.as_path(), data.as_path(), request, claimed.record(),
                        )?),
                        CreationInput::GuardedRecovery => None,
                        CreationInput::Recover(_) => return Err(Error::NotRunning.into()),
                    };
                    // Original path/result and caller roots exist BEFORE notify.
                    reply.custody_requested.get_or_init(|| ());
                    shared.owner_changed.notify_one();
                    #[cfg(test)] reply.test_custody_notified.get_or_init(|| ());
                    if !reply.custody_gate.wait() { return Err(Error::NotRunning.into()); }
                    let custody = claimed.record().connection_custody()
                        .cloned().ok_or(Error::NotRunning)?;
                    match prepared {
                        Some(original) => original.open_and_spawn(&custody, claimed),
                        None => crate::campaigns::recover_guarded_owned(data.as_path(), claimed),
                    }
                });
                CreatorExit::Produced
            });
            self.started[index] = true;
            self.guarded_started[index] = true;
            let open = {
                let admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
                let open = !admission.closed;
                self.shared.replies[index].gate.decide(open);
                if !open { self.shared.replies[index].custody_gate.decide(false); }
                open
            };
            if !open { let _ = self.writers[index].cancel_before_release(); }
            self.shared.replies[index].gate.wake(); // Actual task handle held.
            self.shared.replies[index].custody_gate.wake();
        }
        Ok(())
    }

    /// Preparation must have published its original path AND reached the
    /// custody gate. An early path slot alone is not a completed preparation.
    pub(crate) fn require_custody_window(&self, key: CreatorKey) -> Result<(), IngressRefusal> {
        let index = self.owned_index(key)?;
        if !self.guarded_started[index] || self.shared.replies[index].custody_requested.get().is_none() {
            return Err(IngressRefusal::MissingAttempt);
        }
        if self.shared.close_request.get().is_some() { return Err(IngressRefusal::Closed); }
        Ok(())
    }

    /// Main calls only AFTER successful Host Arc publication. Closure orders
    /// gate decision under admission lock; no wake/mutex spans Host calls.
    pub(crate) fn release_guarded_after_mint(&self, key: CreatorKey) -> Result<(), IngressRefusal> {
        let index = self.owned_index(key)?;
        let open = {
            let admission = self.shared.admission.lock().map_err(|_| IngressRefusal::Poisoned)?;
            let open = !admission.closed && self.writers[index].record().connection_custody().is_some();
            self.shared.replies[index].custody_gate.decide(open);
            open
        };
        self.shared.replies[index].custody_gate.wake();
        if open { Ok(()) } else { Err(IngressRefusal::Closed) }
    }

    pub(crate) fn poll_guarded_creators(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), IngressRefusal>> {
        if let Err(original) = self.dispatch_guarded_pending() { return Poll::Ready(Err(original)); }
        self.poll_started_creators(cx, true)
    }
    /// Poll ONLY the actual retained selected task. RetainedJoin stores its
    /// complete output BEFORE status/notification. No child task/Host borrow.
    fn poll_selected_creator(&mut self, index: usize, cx: &mut Context<'_>)
        -> Poll<Result<(), PreparedAccessRefusal>>
    {
        match self.tasks[index].poll_completion(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(_)) => Poll::Ready(Err(PreparedAccessRefusal::Ingress(IngressRefusal::MissingAttempt))),
            Poll::Ready(Ok(())) => {
                let status = match self.tasks[index].observe() {
                    JoinObservation::Returned { original: Ok(CreatorExit::Produced), was_unknown: false } => RETURNED,
                    JoinObservation::Returned { original: Ok(CreatorExit::CancelledBeforeEffects), was_unknown: false } => NOT_STARTED,
                    JoinObservation::Returned { original: Err(_), was_unknown: false } => JOIN_FAILED,
                    _ => UNKNOWN,
                };
                self.publish_status(index, status);
                Poll::Ready(Ok(()))
            }
        }
    }

    fn selected_terminal_refusal(&self, index: usize) -> PreparedAccessRefusal {
        match self.tasks[index].observe() {
            JoinObservation::Returned { original: Err(_), was_unknown: false } => PreparedAccessRefusal::CreatorJoinFailed,
            JoinObservation::Returned { was_unknown: true, .. } | JoinObservation::Unknown => PreparedAccessRefusal::UnknownCreator,
            _ => PreparedAccessRefusal::UnsuccessfulProduct,
        }
    }

    /// Await actual preparation, never call Host speculatively or retry a
    /// PreparedUnavailable mint. Cancellation drops ONLY this mutable borrow.
    pub(crate) async fn wait_guarded_preparation(&mut self, key: CreatorKey)
        -> Result<(), PreparedAccessRefusal>
    {
        let index = self.dispatch_guarded_selected(key).map_err(PreparedAccessRefusal::Ingress)?;
        loop {
            if self.close_result().is_some() { return Err(PreparedAccessRefusal::Ingress(IngressRefusal::Closed)); }
            match self.tasks[index].observe() {
                JoinObservation::Unknown | JoinObservation::Returned { .. } => return Err(self.selected_terminal_refusal(index)),
                JoinObservation::Pending => (),
            }
            if self.shared.replies[index].custody_requested.get().is_some() { return Ok(()); }
            let observer = self.observer();
            tokio::select! {
                result = std::future::poll_fn(|cx| self.poll_selected_creator(index, cx)) => {
                    result?;
                    return Err(self.selected_terminal_refusal(index));
                }
                _ = observer.owner_changed() => (),
            }
        }
    }

    /// After mint released the same task, await its REAL returned product and
    /// held raw join before entering the synchronous pair cut. No GO here.
    pub(crate) async fn wait_guarded_writer_parked(&mut self, key: CreatorKey)
        -> Result<(), PreparedAccessRefusal>
    {
        let index = self.owned_index(key).map_err(PreparedAccessRefusal::Ingress)?;
        if !self.guarded_started[index] { return Err(PreparedAccessRefusal::UnsuccessfulProduct); }
        loop {
            if self.close_result().is_some() { return Err(PreparedAccessRefusal::Ingress(IngressRefusal::Closed)); }
            match self.tasks[index].observe() {
                JoinObservation::Pending => (),
                _ => {
                    self.prepared_binding(key)?;
                    self.require_pairing_window(key).map_err(PreparedAccessRefusal::Ingress)?;
                    return Ok(());
                }
            }
            let observer = self.observer();
            tokio::select! {
                result = std::future::poll_fn(|cx| self.poll_selected_creator(index, cx)) => {
                    result?;
                    self.prepared_binding(key)?;
                    self.require_pairing_window(key).map_err(PreparedAccessRefusal::Ingress)?;
                    return Ok(());
                }
                _ = observer.owner_changed() => (),
            }
        }
    }

}

#[cfg(test)]
#[path = "owned_preparation_controls.rs"]
mod controls;

#[cfg(test)]
#[path = "owned_phase_wait_controls.rs"]
mod phase_wait_controls;
