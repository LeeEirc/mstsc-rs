use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgAction, Parser};

use crate::config::{ConnectionOverrides, PasswordSource, SecretString, SessionConfig};
use crate::error::{Error, Result};

/// Command line accepted by `mstsc-rs`.
#[derive(Debug, Parser)]
#[command(
    name = "mstsc-rs",
    version,
    about = "Native RDP client using the Windows Remote Desktop ActiveX control",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// RDP file to load. Command-line values override its properties.
    #[arg(value_name = "FILE.rdp")]
    pub rdp_file: Option<PathBuf>,

    /// Remote host, optionally including :port.
    #[arg(short = 'v', long)]
    pub server: Option<String>,

    #[arg(short = 'u', long)]
    pub username: Option<String>,

    #[arg(long)]
    pub domain: Option<String>,

    /// Clear-text password. Prefer --password-env where possible.
    #[arg(long, conflicts_with = "password_env")]
    pub password: Option<String>,

    /// Read the password from NAME (default: MSTSC_RS_PASSWORD).
    #[arg(
        long,
        value_name = "NAME",
        num_args = 0..=1,
        default_missing_value = "MSTSC_RS_PASSWORD"
    )]
    pub password_env: Option<String>,

    #[arg(long)]
    pub width: Option<u32>,

    #[arg(long)]
    pub height: Option<u32>,

    #[arg(short = 'f', long, action = ArgAction::SetTrue)]
    pub fullscreen: bool,

    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "span")]
    pub multimon: bool,

    #[arg(long, action = ArgAction::SetTrue, conflicts_with = "multimon")]
    pub span: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub admin: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub public: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub restricted_admin: bool,

    #[arg(long, action = ArgAction::SetTrue)]
    pub remote_guard: bool,

    #[arg(long)]
    pub gateway: Option<String>,

    #[arg(long, action = ArgAction::SetTrue)]
    pub dynamic_resolution: bool,

    #[arg(long)]
    pub remote_app: Option<String>,

    #[arg(long)]
    pub remote_app_args: Option<String>,

    #[arg(long)]
    pub remote_app_workdir: Option<String>,

    #[arg(long, action = ArgAction::Set)]
    pub redirect_clipboard: Option<bool>,

    #[arg(long, action = ArgAction::Set)]
    pub redirect_printers: Option<bool>,

    /// Drive list (`*`, `C:;D:`, or empty to disable).
    #[arg(long)]
    pub redirect_drives: Option<String>,

    #[arg(long, action = ArgAction::Set)]
    pub redirect_smartcards: Option<bool>,

    #[arg(long, action = ArgAction::Set)]
    pub redirect_com_ports: Option<bool>,

    #[arg(long, action = ArgAction::Set)]
    pub redirect_webauthn: Option<bool>,

    #[arg(long, action = ArgAction::Set)]
    pub redirect_location: Option<bool>,

    /// Camera selector accepted by the RDP `camerastoredirect` property.
    #[arg(long)]
    pub redirect_cameras: Option<String>,

    #[arg(long, action = ArgAction::Set)]
    pub redirect_microphone: Option<bool>,

    /// 0: play locally, 1: play remotely, 2: disable.
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=2))]
    pub audio_mode: Option<u32>,

    /// Set or replace any RDP property as `name:type:value`. Repeatable.
    #[arg(long = "set", value_name = "NAME:TYPE:VALUE")]
    pub custom_properties: Vec<String>,

    #[arg(long)]
    pub title: Option<String>,

    /// Validate and print the merged settings without opening a connection.
    #[arg(long, action = ArgAction::SetTrue)]
    pub dry_run: bool,
}

impl Cli {
    /// Process-oriented parser. Help, version and syntax errors are printed in
    /// clap's normal form and terminate with the appropriate exit code.
    pub fn parse_compatible<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let normalized = normalize_mstsc_args(args);
        Self::try_parse_from(normalized).unwrap_or_else(|error| error.exit())
    }

    /// Parses both ordinary GNU options and common `mstsc.exe` slash switches.
    pub fn try_parse_compatible<I, T>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let normalized = normalize_mstsc_args(args);
        Self::try_parse_from(normalized).map_err(|error| Error::CommandLine(error.to_string()))
    }

    pub fn into_session_config(self) -> Result<(SessionConfig, bool)> {
        let password = match (self.password, self.password_env.as_deref()) {
            (Some(password), _) => Some((SecretString::new(password), PasswordSource::CommandLine)),
            (None, Some(name)) => match std::env::var(name) {
                Ok(password) => Some((
                    SecretString::new(password),
                    PasswordSource::Environment(name.to_owned()),
                )),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(Error::NonUnicodePasswordEnvironment(name.to_owned()));
                }
            },
            (None, None) => match std::env::var("MSTSC_RS_PASSWORD") {
                Ok(password) => Some((
                    SecretString::new(password),
                    PasswordSource::Environment("MSTSC_RS_PASSWORD".to_owned()),
                )),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(Error::NonUnicodePasswordEnvironment(
                        "MSTSC_RS_PASSWORD".to_owned(),
                    ));
                }
            },
        };

        let (password, password_source) = password
            .map(|(password, source)| (Some(password), Some(source)))
            .unwrap_or_default();

        let overrides = ConnectionOverrides {
            server: self.server,
            username: self.username,
            domain: self.domain,
            password,
            password_source,
            width: self.width,
            height: self.height,
            fullscreen: self.fullscreen.then_some(true),
            multimon: self.multimon.then_some(true),
            span: self.span.then_some(true),
            admin: self.admin.then_some(true),
            public_mode: self.public.then_some(true),
            restricted_admin: self.restricted_admin.then_some(true),
            remote_guard: self.remote_guard.then_some(true),
            gateway: self.gateway,
            dynamic_resolution: self.dynamic_resolution.then_some(true),
            remote_app: self.remote_app,
            remote_app_args: self.remote_app_args,
            remote_app_workdir: self.remote_app_workdir,
            redirect_clipboard: self.redirect_clipboard,
            redirect_printers: self.redirect_printers,
            redirect_drives: self.redirect_drives,
            redirect_smartcards: self.redirect_smartcards,
            redirect_com_ports: self.redirect_com_ports,
            redirect_webauthn: self.redirect_webauthn,
            redirect_location: self.redirect_location,
            redirect_cameras: self.redirect_cameras,
            redirect_microphone: self.redirect_microphone,
            audio_mode: self.audio_mode,
            custom_properties: self.custom_properties,
            title: self.title,
        };
        Ok((
            SessionConfig::resolve(self.rdp_file, overrides)?,
            self.dry_run,
        ))
    }
}

fn normalize_mstsc_args<I, T>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    args.into_iter()
        .map(Into::into)
        .map(|arg| {
            let Some(text) = arg.to_str() else {
                return arg;
            };
            let lower = text.to_ascii_lowercase();
            let mapped = match lower.as_str() {
                "/f" => Some("--fullscreen".to_owned()),
                "/multimon" => Some("--multimon".to_owned()),
                "/span" => Some("--span".to_owned()),
                "/admin" | "/console" => Some("--admin".to_owned()),
                "/public" => Some("--public".to_owned()),
                "/restrictedadmin" => Some("--restricted-admin".to_owned()),
                "/remoteguard" => Some("--remote-guard".to_owned()),
                "/?" | "/help" => Some("--help".to_owned()),
                _ if lower.starts_with("/v:") => Some(format!("--server={}", &text[3..])),
                _ if lower.starts_with("/w:") => Some(format!("--width={}", &text[3..])),
                _ if lower.starts_with("/h:") => Some(format!("--height={}", &text[3..])),
                _ => None,
            };
            mapped.map(OsString::from).unwrap_or(arg)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_mstsc_slash_switches() {
        let cli = Cli::try_parse_compatible([
            "mstsc-rs",
            "sample.rdp",
            "/v:host.example:3390",
            "/f",
            "/multimon",
            "/w:1920",
            "/h:1080",
        ])
        .unwrap();
        assert_eq!(cli.rdp_file, Some(PathBuf::from("sample.rdp")));
        assert_eq!(cli.server.as_deref(), Some("host.example:3390"));
        assert!(cli.fullscreen);
        assert!(cli.multimon);
        assert_eq!(cli.width, Some(1920));
        assert_eq!(cli.height, Some(1080));
    }

    #[test]
    fn rejects_span_with_multimon() {
        assert!(Cli::try_parse_compatible(["mstsc-rs", "/span", "/multimon"]).is_err());
    }
}
