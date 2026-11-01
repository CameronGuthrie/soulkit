//! Soulframe item-catalogue pipelines (Rust port of the helpers in
//! `soulframe-research`):
//!
//! - [`all_items`] — port of `generate_all_items.py`.
//! - [`reward_distributions`] — port of `generate_reward_distributions.py`.

pub mod all_items;
pub mod cli;
pub mod format_packages;
pub mod reward_distributions;
