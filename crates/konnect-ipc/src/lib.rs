#[allow(clippy::all, warnings)]
pub mod gen;

pub mod builders;
pub mod client;
pub mod types;

pub use client::{IpcError, KiCadIpcClient};
pub use types::*;
