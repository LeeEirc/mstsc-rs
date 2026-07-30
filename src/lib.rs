//! Reusable configuration and Windows host library for `mstsc-rs`.
//!
//! Parsing and option merging are platform independent. The actual ActiveX host
//! is only available when targeting Windows.

pub mod cli;
pub mod config;
pub mod error;
pub mod rdp;

#[cfg(windows)]
pub mod windows;

pub use config::{ConnectionOverrides, PasswordSource, SessionConfig};
pub use error::{Error, Result};
pub use rdp::{RdpDocument, RdpEncoding, RdpEntry, RdpLine, RdpValue};
