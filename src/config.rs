use std::path::PathBuf;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::Result;
use crate::rdp::RdpDocument;

#[derive(Clone, Debug, Default, Zeroize, ZeroizeOnDrop)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PasswordSource {
    CommandLine,
    Environment(String),
    Interactive,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectionOverrides {
    pub server: Option<String>,
    pub username: Option<String>,
    pub domain: Option<String>,
    pub password: Option<SecretString>,
    pub password_source: Option<PasswordSource>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fullscreen: Option<bool>,
    pub multimon: Option<bool>,
    pub span: Option<bool>,
    pub admin: Option<bool>,
    pub public_mode: Option<bool>,
    pub restricted_admin: Option<bool>,
    pub remote_guard: Option<bool>,
    pub gateway: Option<String>,
    pub dynamic_resolution: Option<bool>,
    pub remote_app: Option<String>,
    pub remote_app_args: Option<String>,
    pub remote_app_workdir: Option<String>,
    pub redirect_clipboard: Option<bool>,
    pub redirect_printers: Option<bool>,
    pub redirect_drives: Option<String>,
    pub redirect_smartcards: Option<bool>,
    pub redirect_com_ports: Option<bool>,
    pub redirect_webauthn: Option<bool>,
    pub redirect_location: Option<bool>,
    pub redirect_cameras: Option<String>,
    pub redirect_microphone: Option<bool>,
    pub audio_mode: Option<u32>,
    pub custom_properties: Vec<String>,
    pub title: Option<String>,
}

/// Fully merged settings. `document` is the authoritative settings stream sent
/// to the system RDP control.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub source_path: Option<PathBuf>,
    pub document: RdpDocument,
    pub server: Option<String>,
    pub username: Option<String>,
    pub domain: Option<String>,
    pub password: Option<SecretString>,
    pub password_source: Option<PasswordSource>,
    pub has_embedded_password: bool,
    pub dynamic_resolution: bool,
    pub fullscreen: bool,
    pub span: bool,
    pub title: String,
}

impl SessionConfig {
    pub fn resolve(source_path: Option<PathBuf>, overrides: ConnectionOverrides) -> Result<Self> {
        let mut document = match source_path.as_ref() {
            Some(path) => RdpDocument::load(path)?,
            None => default_document(),
        };

        apply_overrides(&mut document, &overrides)?;

        let server = document
            .get_string("full address")
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty());
        let username = document
            .get_string("username")
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty());
        let domain = document
            .get_string("domain")
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty());
        let has_embedded_password =
            document.contains("password 51") || document.contains("password");
        // Windows' RDP clients default to following the local window size when
        // the property is absent. Preserve an explicit opt-out from an .rdp
        // file, but make ordinary sessions responsive by default.
        let span = document.get_integer("span monitors") == Some(1);
        let dynamic_resolution = document.get_integer("dynamic resolution") != Some(0) && !span;
        let fullscreen = document.get_integer("screen mode id") == Some(2);
        let title = overrides.title.unwrap_or_else(|| match server.as_deref() {
            Some(server) => format!("{server} - mstsc-rs"),
            None => "mstsc-rs".to_owned(),
        });

        Ok(Self {
            source_path,
            document,
            server,
            username,
            domain,
            password: overrides.password,
            password_source: overrides.password_source,
            has_embedded_password,
            dynamic_resolution,
            fullscreen,
            span,
            title,
        })
    }

    pub fn needs_interactive_input(&self) -> bool {
        self.server.is_none()
    }

    pub fn apply_interactive(
        &mut self,
        server: String,
        username: Option<String>,
        domain: Option<String>,
        password: Option<SecretString>,
    ) {
        self.document.set_string("full address", &server);
        if let Some(username) = username.as_deref() {
            self.document.set_string("username", username);
        } else {
            self.document.remove_all("username");
        }
        if let Some(domain) = domain.as_deref() {
            self.document.set_string("domain", domain);
        } else {
            self.document.remove_all("domain");
        }
        self.server = Some(server);
        self.username = username;
        self.domain = domain;
        self.password = password;
        self.password_source = self.password.as_ref().map(|_| PasswordSource::Interactive);
    }

    /// Produces the setting stream without adding a clear-text password.
    pub fn rdp_settings_text(&self) -> String {
        self.document.render()
    }
}

fn default_document() -> RdpDocument {
    let mut document = RdpDocument::default();
    document.set_integer("screen mode id", 1);
    document.set_integer("dynamic resolution", 1);
    document.set_integer("session bpp", 32);
    document.set_integer("compression", 1);
    document.set_integer("networkautodetect", 1);
    document.set_integer("bandwidthautodetect", 1);
    document.set_integer("enablecredsspsupport", 1);
    document.set_integer("authentication level", 2);
    document.set_integer("prompt for credentials", 0);
    document.set_integer("redirectclipboard", 1);
    document.set_integer("redirectprinters", 1);
    document.set_integer("redirectsmartcards", 1);
    document.set_integer("redirectwebauthn", 1);
    document.set_integer("audiomode", 0);
    document.set_integer("audiocapturemode", 1);
    document
}

fn apply_overrides(document: &mut RdpDocument, overrides: &ConnectionOverrides) -> Result<()> {
    set_string(document, "full address", overrides.server.as_deref());
    set_string(document, "username", overrides.username.as_deref());
    set_string(document, "domain", overrides.domain.as_deref());
    set_u32(document, "desktopwidth", overrides.width);
    set_u32(document, "desktopheight", overrides.height);
    set_bool_as(document, "screen mode id", overrides.fullscreen, 2, 1);
    set_bool(document, "use multimon", overrides.multimon);
    set_bool(document, "span monitors", overrides.span);
    if overrides.span == Some(true) {
        // Legacy span is one large remote desktop across the local virtual
        // desktop, not RDP multiple-monitor mode.
        document.set_integer("screen mode id", 2);
        document.set_integer("use multimon", 0);
    }
    set_bool(document, "administrative session", overrides.admin);
    set_bool(document, "public mode", overrides.public_mode);
    set_bool(
        document,
        "restricted admin mode",
        overrides.restricted_admin,
    );
    set_bool(document, "remote credential guard", overrides.remote_guard);
    set_string(document, "gatewayhostname", overrides.gateway.as_deref());
    if overrides.gateway.is_some() {
        document.set_integer("gatewayusagemethod", 1);
        document.set_integer("gatewayprofileusagemethod", 1);
    }
    set_bool(document, "dynamic resolution", overrides.dynamic_resolution);
    set_bool(document, "redirectclipboard", overrides.redirect_clipboard);
    set_bool(document, "redirectprinters", overrides.redirect_printers);
    set_string(
        document,
        "drivestoredirect",
        overrides.redirect_drives.as_deref(),
    );
    set_bool(
        document,
        "redirectsmartcards",
        overrides.redirect_smartcards,
    );
    set_bool(document, "redirectcomports", overrides.redirect_com_ports);
    set_bool(document, "redirectwebauthn", overrides.redirect_webauthn);
    set_bool(document, "redirectlocation", overrides.redirect_location);
    set_string(
        document,
        "camerastoredirect",
        overrides.redirect_cameras.as_deref(),
    );
    set_bool(document, "audiocapturemode", overrides.redirect_microphone);
    set_u32(document, "audiomode", overrides.audio_mode);

    if let Some(program) = overrides.remote_app.as_deref() {
        document.set_integer("remoteapplicationmode", 1);
        document.set_string("remoteapplicationprogram", program);
    }
    set_string(
        document,
        "remoteapplicationcmdline",
        overrides.remote_app_args.as_deref(),
    );
    set_string(
        document,
        "shell working directory",
        overrides.remote_app_workdir.as_deref(),
    );

    for assignment in &overrides.custom_properties {
        document.set_assignment(assignment)?;
    }
    Ok(())
}

fn set_string(document: &mut RdpDocument, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        document.set_string(key, value);
    }
}

fn set_u32(document: &mut RdpDocument, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        document.set_integer(key, value.min(i32::MAX as u32) as i32);
    }
}

fn set_bool(document: &mut RdpDocument, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        document.set_integer(key, i32::from(value));
    }
}

fn set_bool_as(document: &mut RdpDocument, key: &str, value: Option<bool>, yes: i32, no: i32) {
    if let Some(value) = value {
        document.set_integer(key, if value { yes } else { no });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_overrides_document() {
        let mut original = RdpDocument::parse(
            "full address:s:old\r\nusername:s:file-user\r\nx-private:q:keep\r\n",
        );
        original.set_string("full address", "old");

        let overrides = ConnectionOverrides {
            server: Some("new:3390".to_owned()),
            width: Some(1920),
            ..Default::default()
        };
        apply_overrides(&mut original, &overrides).unwrap();

        assert_eq!(original.get_string("full address"), Some("new:3390"));
        assert_eq!(original.get_integer("desktopwidth"), Some(1920));
        assert!(original.render().contains("x-private:q:keep"));
    }

    #[test]
    fn only_a_missing_server_requires_the_connection_form() {
        let overrides = ConnectionOverrides {
            server: Some("server".into()),
            ..Default::default()
        };
        let config = SessionConfig::resolve(None, overrides).unwrap();
        assert!(!config.needs_interactive_input());

        let missing_server = SessionConfig::resolve(None, ConnectionOverrides::default()).unwrap();
        assert!(missing_server.needs_interactive_input());
    }

    #[test]
    fn dynamic_resolution_defaults_on_and_can_be_disabled() {
        let defaults = SessionConfig::resolve(None, ConnectionOverrides::default()).unwrap();
        assert!(defaults.dynamic_resolution);
        assert_eq!(defaults.document.get_integer("dynamic resolution"), Some(1));

        let disabled = SessionConfig::resolve(
            None,
            ConnectionOverrides {
                dynamic_resolution: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!disabled.dynamic_resolution);
        assert_eq!(disabled.document.get_integer("dynamic resolution"), Some(0));
    }

    #[test]
    fn webauthn_redirection_matches_the_mstsc_default() {
        let config = SessionConfig::resolve(None, ConnectionOverrides::default()).unwrap();
        assert_eq!(config.document.get_integer("redirectwebauthn"), Some(1));
    }

    #[test]
    fn span_is_a_fixed_single_desktop_and_not_multimon() {
        let config = SessionConfig::resolve(
            None,
            ConnectionOverrides {
                span: Some(true),
                multimon: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(config.span);
        assert!(config.fullscreen);
        assert!(!config.dynamic_resolution);
        assert_eq!(config.document.get_integer("span monitors"), Some(1));
        assert_eq!(config.document.get_integer("use multimon"), Some(0));
    }
}
