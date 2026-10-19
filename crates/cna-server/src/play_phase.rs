//! PROPOSED private Server phase functions; not a second public API or caller.
//! These functions do not dispatch unguarded factories or manufacture leases.

use crate::{http::owned::OwnedAppOwner, owned_ingress::CreatorKey};
use cna_seats::{seat_host::{SeatHost, SeatHostLease}, failed_evidence::CampaignArchive};
use std::io::ErrorKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhaseRefusal {
    Closed,
    AttemptUnavailable,
    AlreadyMinted,
    MissingCustody,
    MissingConnectionProvenance,
    PreparedUnavailable,
    MissingAttemptPath,
    Host(ErrorKind),
}

/// Phase M: synchronous exclusive original Host, OUTSIDE the factory worker.
/// Actual path/id is retained by concrete preparation before entry. No new
/// UUID, cwd, normalization, file opener or Host clone is supplied here.
pub(crate) fn mint_before_sqlite<const N: usize, const R: usize>(
    owner: &mut OwnedAppOwner<N, R>, key: CreatorKey,
    host: &mut SeatHost, lease: &SeatHostLease, create_new: bool,
) -> Result<(), PhaseRefusal> {
    let creation = owner.original_creation();
    if creation.close_result().is_some() { return Err(PhaseRefusal::Closed); }
    let record = creation.custody_record(key)
        .map_err(|_| PhaseRefusal::AttemptUnavailable)?;
    if record.connection_custody().is_some() { return Err(PhaseRefusal::AlreadyMinted); }
    creation.require_custody_window(key).map_err(|_| PhaseRefusal::PreparedUnavailable)?;
    let path = record.attempt_path().ok_or(PhaseRefusal::MissingAttemptPath)?;
    let original = host.mint_campaign_custody(lease, path, create_new)
        .map_err(PhaseRefusal::Host)?;
    // This private unique owner is the sole installer; before any SQLite open.
    record.install_connection_custody(original).map_err(|_| PhaseRefusal::AlreadyMinted)?;
    creation.release_guarded_after_mint(key).map_err(|_| PhaseRefusal::Closed)
}

/// Phase P: synchronous exclusive original Host AFTER actual creator return
/// with the real writer connection retained/parked and BOTH original joins held.
/// No GO is released by this function, even after a durable unit acknowledgment.
pub(crate) fn pair_before_go<const N: usize, const R: usize>(
    owner: &mut OwnedAppOwner<N, R>, key: CreatorKey,
    host: &mut SeatHost, lease: &SeatHostLease, archive: &CampaignArchive,
) -> Result<(), PhaseRefusal> {
    let creation = owner.original_creation();
    creation.require_pairing_window(key).map_err(|_| PhaseRefusal::PreparedUnavailable)?;
    let actual = creation.prepared_binding(key).map_err(|_| PhaseRefusal::PreparedUnavailable)?;
    let record = creation.custody_record(key).map_err(|_| PhaseRefusal::AttemptUnavailable)?;
    let custody = record.connection_custody().ok_or(PhaseRefusal::MissingCustody)?;
    if !record.guarded_connection_required() || !matches!(record.connection_provenance(), Some(Ok(()))) {
        return Err(PhaseRefusal::MissingConnectionProvenance);
    }
    // Source-owned actual decoded stored facts; no public Server DTO in Host.
    let campaign = [actual.meta.id.as_str(), actual.meta.rules_profile.as_str()];
    let pins = [actual.pins.rules_profile.as_str(), actual.pins.content_hash.as_str(),
        actual.pins.engine_version.as_str()];
    let retained = record.pair_ack().get_or_init(|| host.pair_campaign_custody(
        lease, custody.as_ref(), campaign, pins, archive,
    ));
    retained.as_ref().map(|_| ()).map_err(|original| PhaseRefusal::Host(*original))
}

/// Proposed async outer cut: borrow stays in main, NEVER in the worker.
/// After readiness returns there is no await between Host mint, parent Arc
/// installation and gate release. Existing synchronous code is reused exactly.
pub(crate) async fn prepare_mint_and_park<const N: usize, const R: usize>(
    owner: &mut OwnedAppOwner<N, R>, key: CreatorKey,
    host: &mut SeatHost, lease: &SeatHostLease, create_new: bool,
) -> Result<(), PhaseRefusal> {
    owner.original_creation().wait_guarded_preparation(key).await.map_err(wait_refusal)?;
    mint_before_sqlite(owner, key, host, lease, create_new)?;
    // Return success only after the REAL creator output is held and its writer
    // is parked. The subsequent exclusive Host pair stays synchronous.
    owner.original_creation().wait_guarded_writer_parked(key).await.map_err(wait_refusal)?;
    Ok(())
}

fn wait_refusal(original: crate::owned_ingress::PreparedAccessRefusal) -> PhaseRefusal {
    use crate::owned_ingress::{PreparedAccessRefusal, IngressRefusal};
    match original {
        PreparedAccessRefusal::Ingress(IngressRefusal::Closed) => PhaseRefusal::Closed,
        PreparedAccessRefusal::Ingress(IngressRefusal::ForeignAttempt | IngressRefusal::MissingAttempt) => PhaseRefusal::AttemptUnavailable,
        _ => PhaseRefusal::PreparedUnavailable,
    }
}
