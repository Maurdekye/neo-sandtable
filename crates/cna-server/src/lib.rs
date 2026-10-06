//! A single campaign writer. The engine is pure; acknowledged state is durable.
pub mod campaign;
pub mod scripted;

pub use campaign::{Binding, Campaign, CampaignStatus, Error, Pins, Receipt};
