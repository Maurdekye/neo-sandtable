//! A single campaign writer. The engine is pure; acknowledged state is durable.
mod auth;
pub mod campaign;
pub use auth::CampaignCapabilities;
pub mod scripted;

pub use campaign::{Binding, Campaign, CampaignStatus, Error, Pins, Receipt, RunBoundary};
pub mod actor;
pub mod campaigns;
pub mod cna;
pub mod http;
pub mod play_owner;
pub mod replay;
pub mod operator_failures;
mod host_gate;
mod host_credit;
mod host_forced;
mod host_reply;
mod writer_fatal;
#[allow(dead_code)] // Held owned source; no positive creation or live wiring yet.
mod owned_retained;
#[allow(dead_code)] // Source stage; production admission remains closed.
mod owned_creation;
#[allow(dead_code)] // Held owned writer source; no production constructor calls it.
mod owned_writer;
#[allow(dead_code)] // Private m80 admission; no live activation or fit claim.
mod owned_row_envelope;
#[allow(dead_code)] // Held concrete ingress; no live App/main activation or fit.
mod owned_ingress;
pub mod sandbox;
pub mod seats;
