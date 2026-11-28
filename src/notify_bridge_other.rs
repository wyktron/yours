//! Tuta notifications on platforms without the Linux session bus.
//!
//! The bridge reads Tuta's desktop notifications off the session bus, which
//! only a Linux desktop provides. Every other build keeps the same API and
//! reports itself unavailable, so Settings shows why instead of a dead switch.

use flectar_mail_core::{Core, models::Settings};
use slint::ComponentHandle as _;

/// What Settings and the services probe show: never connected here.
pub(crate) fn publish(app: &crate::AppWindow) {
    let services = app.global::<crate::AppServices>();
    services.set_tuta_state("unavailable".into());
    services.set_tuta_detail("Tuta notifications need a Linux desktop session bus.".into());
}

pub(crate) fn effective_enabled(_settings: &Settings) -> bool {
    false
}

pub(crate) fn start(
    _runtime: &tokio::runtime::Handle,
    _core: std::sync::Arc<Core>,
    _app: slint::Weak<crate::AppWindow>,
) {
}
