//! Reusable configuration and Windows host library for `mstsc-rs`.
//!
//! This crate is intended to be built and run natively on Windows.

#[cfg(not(windows))]
compile_error!("mstsc-rs only supports native Windows builds");

pub mod cli;
pub mod config;
pub mod error;
pub mod rdp;

pub mod windows;

pub use config::{ConnectionOverrides, PasswordSource, SessionConfig};
pub use error::{Error, Result};
pub use rdp::{RdpDocument, RdpEncoding, RdpEntry, RdpLine, RdpValue};
