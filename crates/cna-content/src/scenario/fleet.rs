//! Typed logistics subset of fleet setup; unrelated fleet data remains raw.
use super::Placement;
use serde::Deserialize;
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FleetLogistics {
    pub axis_convoys: Option<AxisConvoys>,
    pub axis_coastal_shipping: Option<AxisCoastalShipping>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct AxisConvoys {
    #[serde(default)]
    pub pre_game_plan_remaining_start_month: bool,
    pub lanes_allowed: Vec<u8>,
    pub src: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct AxisCoastalShipping {
    pub roster: String,
    pub location: Placement,
    pub src: Vec<String>,
}
