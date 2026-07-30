mod activex;
mod ui;

use crate::{Result, SessionConfig};

/// Runs one native Windows window and one embedded RDP session.
pub fn run(config: SessionConfig) -> Result<()> {
    ui::run(config)
}
