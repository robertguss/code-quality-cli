//! The feature map: what a repo is for and where each feature's code lives.
//! Every entry point must belong to a feature, scope changes are reported,
//! and new features carry a task ID.

pub mod inventory;
pub mod map;

pub const MAP_FILE: &str = "qgate.features.toml";

pub const MAP_RULE: &str = "feature-map";
