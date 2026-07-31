#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

pub mod cli;
mod client;
pub(crate) mod daemon;
pub(crate) mod guest;
mod presentation;
pub mod ssh_config;
mod terminal;
