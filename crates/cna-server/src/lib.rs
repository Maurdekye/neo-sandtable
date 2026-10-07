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
pub mod replay;
pub mod sandbox;
pub mod seats;
