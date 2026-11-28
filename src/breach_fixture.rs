//! Demo breach data for seeing the skull without any network.
//!
//! Off by default: only FLECTAR_BREACH_FIXTURE, naming a sender address, asks
//! for one cached demo breach to be stored at startup. The Flatpak build
//! compiles this module out entirely, and the row is plainly labelled DEMO,
//! so a normal run never makes a breach claim no provider made.

use super::*;
use flectar_mail_core::breaches;

/// Store one demo breach for the address in FLECTAR_BREACH_FIXTURE, then
/// reload the rows so the skull and its popup can be seen. Does nothing unless
/// the variable names a valid sender address. The write is one local row and
/// runs only when that developer variable is set, so it is not put on a task.
pub(crate) fn install(
    runtime: &tokio::runtime::Handle,
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    core: std::sync::Arc<flectar_mail_core::Core>,
) {
    let Some(address) = std::env::var_os("FLECTAR_BREACH_FIXTURE") else {
        return;
    };
    let Some(address) = address
        .to_str()
        .map(str::trim)
        .filter(|address| !address.is_empty())
    else {
        eprintln!("FLECTAR_BREACH_FIXTURE ignored: the value is not a UTF-8 address");
        return;
    };
    if !breaches::should_check_address(address) {
        eprintln!("FLECTAR_BREACH_FIXTURE ignored: {address:?} is not a sender address");
        return;
    }
    let address = address.to_owned();
    match runtime.block_on(core.insert_demo_breach(&address)) {
        Ok(()) => {
            // Loud and address-only: a demo skull must never be blamed on a
            // real lookup, and the line names the address it was for.
            eprintln!("demo breach data inserted for {address}");
            if let Err(error) =
                crate::mail_view_model::refresh_from_source(app, state, runtime, true, &[])
            {
                eprintln!("demo breach data: could not reload the mail list: {error}");
            }
        }
        Err(error) => eprintln!("demo breach data insert failed for {address}: {error}"),
    }
}
