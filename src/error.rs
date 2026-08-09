use std::path::PathBuf;

/// Errors raised before control is handed to the Windows RDP component.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read RDP file {path}: {source}")]
    ReadRdp {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("the RDP file is neither UTF-8 nor UTF-16 text")]
    InvalidRdpEncoding,

    #[error("invalid RDP assignment `{0}`; expected name:type:value")]
    InvalidRdpAssignment(String),

    #[error("invalid integer value `{value}` for RDP property `{key}`")]
    InvalidRdpInteger { key: String, value: String },

    #[error("invalid command line: {0}")]
    CommandLine(String),

    #[error("environment variable `{0}` contains non-Unicode password data")]
    NonUnicodePasswordEnvironment(String),

    #[error("Windows API error while {context}: {source}")]
    Windows {
        context: &'static str,
        #[source]
        source: windows::core::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) trait WindowsContext<T> {
    fn windows_context(self, context: &'static str) -> Result<T>;
}

impl<T> WindowsContext<T> for windows::core::Result<T> {
    fn windows_context(self, context: &'static str) -> Result<T> {
        self.map_err(|source| Error::Windows { context, source })
    }
}
