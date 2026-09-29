//! Wayland: global hotkeys through the GlobalShortcuts portal (GNOME 48+,
//! KDE Plasma, Hyprland). Wayland lets no client grab keys itself.
//!
//! Registering starts a thread that opens a portal session, binds one
//! shortcut per binding (with the hotkey as its preferred trigger, in the
//! XDG shortcuts format), and forwards the portal's `Activated` signals. The
//! desktop decides the final triggers: it may ask the user to confirm them,
//! or let them pick others, and it keeps them per app.
//!
//! Binding happens after [`Hotkeys::register`] has returned (it can wait on
//! that dialog), so its problems cannot be reported as
//! [`HotkeyRegistration::failures`]: they are logged instead. Dropping the
//! registration closes the session, even mid-bind (which ends the dialog),
//! and that releases the shortcuts.

use std::pin::pin;
use std::thread::{self, JoinHandle};

use ashpd::desktop::global_shortcuts::{
    Activated, BindShortcutsOptions, GlobalShortcuts, NewShortcut,
};
use ashpd::desktop::{CreateSessionOptions, Session};
use chartreuse_core::capture::CaptureMode;
use chartreuse_core::{Error, Result};
use futures::channel::oneshot;
use futures::future::{self, Either};
use futures::{Stream, StreamExt};

use crate::event::{self, EventSender, Registration};
use crate::hotkeys::{HotkeyBinding, HotkeyEvent, HotkeyRegistration, Hotkeys};
use crate::linux::logic::keysym::xdg_trigger;
use crate::linux::portal;

/// The Wayland [`Hotkeys`] backend.
#[derive(Debug, Default)]
pub struct WaylandHotkeys;

impl WaylandHotkeys {
    pub fn new() -> Self {
        Self
    }
}

impl Hotkeys for WaylandHotkeys {
    fn register(&self, bindings: &[HotkeyBinding]) -> Result<HotkeyRegistration> {
        let (sender, events) = event::channel();
        let (stop, stopped) = oneshot::channel();
        let bindings = bindings.to_vec();
        let thread = thread::Builder::new()
            .name("chartreuse global shortcuts".into())
            .spawn(move || futures::executor::block_on(serve(&bindings, &sender, stopped)))
            .map_err(|e| Error::Platform(format!("starting the global shortcuts thread: {e}")))?;
        Ok(HotkeyRegistration {
            events,
            failures: Vec::new(),
            registration: Registration::new(Shortcuts {
                stop: Some(stop),
                thread: Some(thread),
            }),
        })
    }
}

/// The portal's id for the shortcut that starts `mode`.
fn shortcut_id(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::Display => "capture-display",
        CaptureMode::Selection => "capture-selection",
    }
}

/// Binds `bindings` and forwards their activations until `stopped` fires.
async fn serve(
    bindings: &[HotkeyBinding],
    sender: &EventSender<HotkeyEvent>,
    mut stopped: oneshot::Receiver<()>,
) {
    let portal = match future::select(&mut stopped, pin!(connect())).await {
        Either::Left(_) => return,
        Either::Right((Ok(portal), _)) => portal,
        Either::Right((Err(error), _)) => {
            unavailable(&error);
            return;
        }
    };
    // Not raced against `stopped`: a session the portal opened without us
    // getting its handle could never be closed. Opening one asks the user
    // nothing, so it does not keep a dropped registration waiting.
    let session = match portal.create_session(CreateSessionOptions::default()).await {
        Ok(session) => session,
        Err(error) => {
            unavailable(&error);
            return;
        }
    };
    // Binding can wait on the user. Stopped or failed, the session is still
    // closed below, so that it cannot keep shortcuts bound (whose
    // activations would reach the next registration's listener too).
    let bound = pin!(bind(&portal, &session, bindings));
    let activated = match future::select(&mut stopped, bound).await {
        Either::Left(_) => None,
        Either::Right((Ok(activated), _)) => Some(activated),
        Either::Right((Err(error), _)) => {
            unavailable(&error);
            None
        }
    };
    if let Some(activated) = activated {
        let mut activated = pin!(activated);
        while let Either::Right((Some(activation), _)) =
            future::select(&mut stopped, activated.next()).await
        {
            let binding = bindings
                .iter()
                .find(|binding| shortcut_id(binding.mode) == activation.shortcut_id());
            if let Some(binding) = binding {
                sender.send(HotkeyEvent {
                    mode: binding.mode,
                    hotkey: binding.hotkey,
                });
            }
        }
    }
    if let Err(error) = session.close().await {
        tracing::debug!("closing the global shortcuts session failed: {error}");
    }
}

/// The GlobalShortcuts portal, once Chartreuse's app id is registered.
async fn connect() -> ashpd::Result<GlobalShortcuts> {
    portal::register_app().await;
    GlobalShortcuts::new().await
}

/// Binds `bindings` in `session` and returns the portal's activations.
async fn bind(
    portal: &GlobalShortcuts,
    session: &Session<GlobalShortcuts>,
    bindings: &[HotkeyBinding],
) -> ashpd::Result<impl Stream<Item = Activated> + use<>> {
    // Listen before binding, so that no activation is missed.
    let activated = portal.receive_activated().await?;
    let triggers: Vec<String> = bindings
        .iter()
        .map(|binding| xdg_trigger(binding.hotkey))
        .collect();
    let shortcuts: Vec<NewShortcut> = bindings
        .iter()
        .zip(&triggers)
        .map(|(binding, trigger)| {
            NewShortcut::new(shortcut_id(binding.mode), binding.mode.to_string())
                .preferred_trigger(trigger.as_str())
        })
        .collect();
    let bound = portal
        .bind_shortcuts(session, &shortcuts, None, BindShortcutsOptions::default())
        .await?
        .response()?;
    for shortcut in bound.shortcuts() {
        tracing::info!(
            "global shortcut {}: {}",
            shortcut.id(),
            shortcut.trigger_description()
        );
    }
    Ok(activated)
}

/// Logs why the global hotkeys are unavailable.
fn unavailable(error: &ashpd::Error) {
    let error = if portal::cancelled(error) {
        Error::Platform("the user declined the global shortcuts".into())
    } else {
        portal::error("registering the global shortcuts", error)
    };
    tracing::warn!("global hotkeys are unavailable: {error}");
}

/// The registration: stops the thread, which closes the session.
struct Shortcuts {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Shortcuts {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Waiting for the session to close keeps a new registration's
        // shortcuts from clashing with this one's.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
