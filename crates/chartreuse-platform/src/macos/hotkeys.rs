//! macOS: global hotkeys through Carbon's `RegisterEventHotKey`, which needs no
//! Accessibility permission.
//!
//! Each [`Hotkeys::register`] call installs one `kEventHotKeyPressed` handler on
//! the application event target and registers every binding with a process-wide
//! unique hotkey id. The handler claims only presses whose id belongs to its own
//! set and passes the rest on (`eventNotHandledErr`), so several registrations
//! can coexist. Dropping the [`Registration`] unregisters the hotkeys, removes the
//! handler and frees its context.
//!
//! Hotkeys are registered with `kEventHotKeyExclusive`, so while ours is active,
//! other processes' non-exclusive registrations of the same combination are not
//! notified. Registration fails with `eventHotKeyExistsErr`, reported as
//! [`Error::HotkeyUnavailable`], when another process holds the combination
//! exclusively or another registration in this process holds it; a
//! non-exclusive registration elsewhere does not make it fail. Shortcuts that
//! macOS itself handles (the system screenshot keys, Spotlight, …) are not
//! Carbon registrations, so they do not fail either, and macOS may keep them.
//!
//! Keys are mapped by position on a US ANSI keyboard (`kVK_*` virtual key codes),
//! matching [`Key`]'s definition. [`Key::PrintScreen`] has no Mac key code (a PC
//! keyboard's Print Screen key arrives as F13), so it is a per-binding failure.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use chartreuse_core::hotkey::{Hotkey, Key, Modifiers};
use chartreuse_core::{Error, Result};

use crate::event::{self, EventSender, Registration};
use crate::hotkeys::{HotkeyBinding, HotkeyEvent, HotkeyRegistration, Hotkeys};

/// The macOS [`Hotkeys`] backend.
#[derive(Debug, Default)]
pub struct MacosHotkeys;

impl MacosHotkeys {
    pub fn new() -> Self {
        Self
    }
}

impl Hotkeys for MacosHotkeys {
    fn register(&self, bindings: &[HotkeyBinding]) -> Result<HotkeyRegistration> {
        let (sender, events) = event::channel();
        let (registered, failures) = Registered::new(bindings, sender)?;
        Ok(HotkeyRegistration {
            events,
            failures,
            registration: Registration::new(registered),
        })
    }
}

/// Hand-written bindings to the parts of Carbon's HIToolbox (Carbon Event
/// Manager) that hotkeys need; `objc2-carbon` has none.
#[allow(non_upper_case_globals, non_snake_case)]
mod ffi {
    use std::ffi::c_void;

    pub type OSStatus = i32;
    pub type OSType = u32;

    #[repr(C)]
    #[derive(Debug)]
    pub struct OpaqueEventHotKeyRef {
        _private: [u8; 0],
    }
    #[repr(C)]
    #[derive(Debug)]
    pub struct OpaqueEventHandlerRef {
        _private: [u8; 0],
    }
    #[repr(C)]
    #[derive(Debug)]
    pub struct OpaqueEventHandlerCallRef {
        _private: [u8; 0],
    }
    #[repr(C)]
    #[derive(Debug)]
    pub struct OpaqueEventRef {
        _private: [u8; 0],
    }
    #[repr(C)]
    #[derive(Debug)]
    pub struct OpaqueEventTargetRef {
        _private: [u8; 0],
    }

    pub type EventHotKeyRef = *mut OpaqueEventHotKeyRef;
    pub type EventHandlerRef = *mut OpaqueEventHandlerRef;
    pub type EventHandlerCallRef = *mut OpaqueEventHandlerCallRef;
    pub type EventRef = *mut OpaqueEventRef;
    pub type EventTargetRef = *mut OpaqueEventTargetRef;
    pub type EventHandlerUPP =
        extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EventHotKeyID {
        pub signature: OSType,
        pub id: u32,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct EventTypeSpec {
        pub eventClass: OSType,
        pub eventKind: u32,
    }

    pub const noErr: OSStatus = 0;
    pub const eventNotHandledErr: OSStatus = -9874;
    pub const eventHotKeyExistsErr: OSStatus = -9878;
    pub const eventHotKeyInvalidErr: OSStatus = -9879;

    pub const kEventClassKeyboard: OSType = u32::from_be_bytes(*b"keyb");
    pub const kEventHotKeyPressed: u32 = 5;
    pub const kEventParamDirectObject: OSType = u32::from_be_bytes(*b"----");
    pub const typeEventHotKeyID: OSType = u32::from_be_bytes(*b"hkid");
    pub const kEventHotKeyExclusive: u32 = 1 << 0;

    // Modifier masks (Events.h).
    pub const cmdKey: u32 = 1 << 8;
    pub const shiftKey: u32 = 1 << 9;
    pub const optionKey: u32 = 1 << 11;
    pub const controlKey: u32 = 1 << 12;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        pub fn GetApplicationEventTarget() -> EventTargetRef;
        pub fn RegisterEventHotKey(
            inHotKeyCode: u32,
            inHotKeyModifiers: u32,
            inHotKeyID: EventHotKeyID,
            inTarget: EventTargetRef,
            inOptions: u32,
            outRef: *mut EventHotKeyRef,
        ) -> OSStatus;
        pub fn UnregisterEventHotKey(inHotKey: EventHotKeyRef) -> OSStatus;
        pub fn InstallEventHandler(
            inTarget: EventTargetRef,
            inHandler: EventHandlerUPP,
            inNumTypes: usize,
            inList: *const EventTypeSpec,
            inUserData: *mut c_void,
            outRef: *mut EventHandlerRef,
        ) -> OSStatus;
        pub fn RemoveEventHandler(inHandlerRef: EventHandlerRef) -> OSStatus;
        pub fn GetEventParameter(
            inEvent: EventRef,
            inName: OSType,
            inDesiredType: OSType,
            outActualType: *mut OSType,
            inBufferSize: usize,
            outActualSize: *mut usize,
            outData: *mut c_void,
        ) -> OSStatus;
    }
}

use ffi::{
    EventHandlerCallRef, EventHandlerRef, EventHotKeyID, EventHotKeyRef, EventRef, OSStatus,
};

/// The `EventHotKeyID` signature of Chartreuse's hotkeys.
const SIGNATURE: ffi::OSType = u32::from_be_bytes(*b"Chrt");

/// The Carbon virtual key code (`kVK_*`, Events.h) of `key`, or `None` if Mac
/// keyboards have no such key.
///
/// PC keyboards' Insert key arrives as `kVK_Help`, where Mac extended keyboards
/// have Help, so [`Key::Insert`] maps to it. [`Key::Backspace`] is the Mac
/// Delete key (`kVK_Delete`) and [`Key::Delete`] the forward delete key.
fn key_code(key: Key) -> Option<u32> {
    Some(match key {
        Key::A => 0x00,
        Key::S => 0x01,
        Key::D => 0x02,
        Key::F => 0x03,
        Key::H => 0x04,
        Key::G => 0x05,
        Key::Z => 0x06,
        Key::X => 0x07,
        Key::C => 0x08,
        Key::V => 0x09,
        Key::B => 0x0B,
        Key::Q => 0x0C,
        Key::W => 0x0D,
        Key::E => 0x0E,
        Key::R => 0x0F,
        Key::Y => 0x10,
        Key::T => 0x11,
        Key::Digit1 => 0x12,
        Key::Digit2 => 0x13,
        Key::Digit3 => 0x14,
        Key::Digit4 => 0x15,
        Key::Digit6 => 0x16,
        Key::Digit5 => 0x17,
        Key::Equal => 0x18,
        Key::Digit9 => 0x19,
        Key::Digit7 => 0x1A,
        Key::Minus => 0x1B,
        Key::Digit8 => 0x1C,
        Key::Digit0 => 0x1D,
        Key::RightBracket => 0x1E,
        Key::O => 0x1F,
        Key::U => 0x20,
        Key::LeftBracket => 0x21,
        Key::I => 0x22,
        Key::P => 0x23,
        Key::Return => 0x24,
        Key::L => 0x25,
        Key::J => 0x26,
        Key::Quote => 0x27,
        Key::K => 0x28,
        Key::Semicolon => 0x29,
        Key::Backslash => 0x2A,
        Key::Comma => 0x2B,
        Key::Slash => 0x2C,
        Key::N => 0x2D,
        Key::M => 0x2E,
        Key::Period => 0x2F,
        Key::Tab => 0x30,
        Key::Space => 0x31,
        Key::Grave => 0x32,
        Key::Backspace => 0x33,
        Key::Escape => 0x35,
        Key::F17 => 0x40,
        Key::F18 => 0x4F,
        Key::F19 => 0x50,
        Key::F20 => 0x5A,
        Key::F5 => 0x60,
        Key::F6 => 0x61,
        Key::F7 => 0x62,
        Key::F3 => 0x63,
        Key::F8 => 0x64,
        Key::F9 => 0x65,
        Key::F11 => 0x67,
        Key::F13 => 0x69,
        Key::F16 => 0x6A,
        Key::F14 => 0x6B,
        Key::F10 => 0x6D,
        Key::F12 => 0x6F,
        Key::F15 => 0x71,
        Key::Insert => 0x72,
        Key::Home => 0x73,
        Key::PageUp => 0x74,
        Key::Delete => 0x75,
        Key::F4 => 0x76,
        Key::End => 0x77,
        Key::F2 => 0x78,
        Key::PageDown => 0x79,
        Key::F1 => 0x7A,
        Key::Left => 0x7B,
        Key::Right => 0x7C,
        Key::Down => 0x7D,
        Key::Up => 0x7E,
        Key::PrintScreen => return None,
    })
}

/// The Carbon modifier mask of `modifiers` (`Super` is Command).
fn modifier_mask(modifiers: Modifiers) -> u32 {
    [
        (Modifiers::CONTROL, ffi::controlKey),
        (Modifiers::ALT, ffi::optionKey),
        (Modifiers::SHIFT, ffi::shiftKey),
        (Modifiers::SUPER, ffi::cmdKey),
    ]
    .into_iter()
    .filter(|&(modifier, _)| modifiers.contains(modifier))
    .fold(0, |mask, (_, bit)| mask | bit)
}

/// The Carbon `(key code, modifier mask)` of `hotkey`, or why it cannot be
/// registered.
fn carbon_combination(hotkey: Hotkey) -> std::result::Result<(u32, u32), Error> {
    match key_code(hotkey.key) {
        Some(code) => Ok((code, modifier_mask(hotkey.modifiers))),
        None => Err(Error::HotkeyUnavailable {
            hotkey,
            reason: format!("Mac keyboards have no {} key", hotkey.key.name()),
        }),
    }
}

/// Why `RegisterEventHotKey` refused `hotkey` with `status`.
fn registration_error(hotkey: Hotkey, status: OSStatus) -> Error {
    let reason = match status {
        ffi::eventHotKeyExistsErr => {
            "another app or another Chartreuse hotkey is already using it".to_owned()
        }
        ffi::eventHotKeyInvalidErr => "macOS rejected it as invalid".to_owned(),
        status => format!("RegisterEventHotKey failed with OSStatus {status}"),
    };
    Error::HotkeyUnavailable { hotkey, reason }
}

/// Gives every Carbon hotkey in the process its own id, so each handler can tell
/// its own presses from other registrations'.
fn next_hotkey_id() -> u32 {
    static NEXT_ID: AtomicU32 = AtomicU32::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// What the event handler needs: the registered bindings by hotkey id, and where
/// to send their presses.
#[derive(Debug)]
struct Context {
    sender: EventSender<HotkeyEvent>,
    bindings: Vec<(u32, HotkeyBinding)>,
}

impl Context {
    /// Sends the press of hotkey `id` if it is one of ours. Returns whether it was.
    fn deliver(&self, id: EventHotKeyID) -> bool {
        if id.signature != SIGNATURE {
            return false;
        }
        let Some(&(_, binding)) = self.bindings.iter().find(|(ours, _)| *ours == id.id) else {
            return false;
        };
        // A closed receiver just means the app is not listening any more.
        self.sender.send(HotkeyEvent {
            mode: binding.mode,
            hotkey: binding.hotkey,
        });
        true
    }
}

/// One registered set of hotkeys and its event handler. Dropping it undoes both.
///
/// Built and dropped on the main thread (the raw pointers make it `!Send`).
#[derive(Debug)]
struct Registered {
    hotkeys: Vec<EventHotKeyRef>,
    /// Null until the handler is installed.
    handler: EventHandlerRef,
    /// The handler's user data, from `Box::into_raw`; null until installed.
    context: *mut Context,
}

impl Registered {
    /// Registers `bindings` and installs the handler that sends their presses to
    /// `sender`. Bindings that cannot be registered are returned as failures; an
    /// `Err` means hotkeys are unavailable altogether.
    fn new(
        bindings: &[HotkeyBinding],
        sender: EventSender<HotkeyEvent>,
    ) -> Result<(Self, Vec<(HotkeyBinding, Error)>)> {
        let mut registered = Self {
            hotkeys: Vec::new(),
            handler: ptr::null_mut(),
            context: ptr::null_mut(),
        };
        let mut active = Vec::new();
        let mut failures = Vec::new();
        for &binding in bindings {
            match registered.register(binding.hotkey) {
                Ok(id) => active.push((id, binding)),
                Err(error) => failures.push((binding, error)),
            }
        }
        // No event is dispatched before we return to the run loop, so installing
        // the handler after the hotkeys loses no presses. On failure, dropping
        // `registered` unregisters the hotkeys.
        registered.install_handler(Context {
            sender,
            bindings: active,
        })?;
        Ok((registered, failures))
    }

    /// Registers one hotkey and returns its id.
    fn register(&mut self, hotkey: Hotkey) -> Result<u32> {
        let (code, modifiers) = carbon_combination(hotkey)?;
        let id = next_hotkey_id();
        let mut hotkey_ref: EventHotKeyRef = ptr::null_mut();
        // SAFETY: Plain values in, one out-pointer to a live local. Called on the
        // main thread (the `Hotkeys` contract), as Carbon requires.
        let status = unsafe {
            ffi::RegisterEventHotKey(
                code,
                modifiers,
                EventHotKeyID {
                    signature: SIGNATURE,
                    id,
                },
                ffi::GetApplicationEventTarget(),
                ffi::kEventHotKeyExclusive,
                &raw mut hotkey_ref,
            )
        };
        if status != ffi::noErr {
            return Err(registration_error(hotkey, status));
        }
        self.hotkeys.push(hotkey_ref);
        Ok(id)
    }

    fn install_handler(&mut self, context: Context) -> Result<()> {
        self.context = Box::into_raw(Box::new(context));
        let spec = ffi::EventTypeSpec {
            eventClass: ffi::kEventClassKeyboard,
            eventKind: ffi::kEventHotKeyPressed,
        };
        // SAFETY: `spec` outlives the call (Carbon copies the list). The user data
        // is `self.context`, which stays valid until `drop` has removed the
        // handler. `on_hotkey_pressed` matches `EventHandlerUPP`.
        let status = unsafe {
            ffi::InstallEventHandler(
                ffi::GetApplicationEventTarget(),
                on_hotkey_pressed,
                1,
                &raw const spec,
                self.context.cast(),
                &raw mut self.handler,
            )
        };
        if status == ffi::noErr {
            Ok(())
        } else {
            self.handler = ptr::null_mut();
            Err(Error::Platform(format!(
                "could not install the hotkey event handler (OSStatus {status})"
            )))
        }
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        for hotkey in self.hotkeys.drain(..) {
            // SAFETY: `hotkey` came from a successful `RegisterEventHotKey` and is
            // unregistered exactly once, on the main thread.
            let status = unsafe { ffi::UnregisterEventHotKey(hotkey) };
            if status != ffi::noErr {
                tracing::warn!(status, "UnregisterEventHotKey failed");
            }
        }
        if !self.handler.is_null() {
            // SAFETY: `handler` came from a successful `InstallEventHandler` and is
            // removed exactly once. Afterwards Carbon no longer uses the context.
            let status = unsafe { ffi::RemoveEventHandler(self.handler) };
            if status != ffi::noErr {
                tracing::warn!(status, "RemoveEventHandler failed");
            }
        }
        if !self.context.is_null() {
            // SAFETY: `context` came from `Box::into_raw` and is freed exactly once,
            // after the handler that used it is gone.
            drop(unsafe { Box::from_raw(self.context) });
        }
    }
}

/// The `kEventHotKeyPressed` handler. Claims the event if the pressed hotkey
/// belongs to this handler's registration, and otherwise passes it on to the
/// next handler (another registration's).
extern "C" fn on_hotkey_pressed(
    _call: EventHandlerCallRef,
    event: EventRef,
    user_data: *mut c_void,
) -> OSStatus {
    let mut id = EventHotKeyID {
        signature: 0,
        id: 0,
    };
    // SAFETY: `event` is the event being dispatched; the out-buffer is a live
    // `EventHotKeyID` of the size passed in.
    let status = unsafe {
        ffi::GetEventParameter(
            event,
            ffi::kEventParamDirectObject,
            ffi::typeEventHotKeyID,
            ptr::null_mut(),
            size_of::<EventHotKeyID>(),
            ptr::null_mut(),
            (&raw mut id).cast(),
        )
    };
    if status != ffi::noErr {
        return ffi::eventNotHandledErr;
    }
    // SAFETY: `user_data` is the `Context` of the `Registered` that installed this
    // handler, which frees it only after removing the handler. Handlers run on the
    // main thread, where the context is otherwise only read.
    let context = unsafe { &*user_data.cast::<Context>() };
    if context.deliver(id) {
        ffi::noErr
    } else {
        ffi::eventNotHandledErr
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chartreuse_core::capture::CaptureMode;
    use parking_lot::Mutex;

    use super::*;

    #[test]
    fn every_key_but_print_screen_has_a_distinct_key_code() {
        let mut codes: HashMap<u32, Key> = HashMap::new();
        for &key in Key::ALL {
            match key_code(key) {
                Some(code) => {
                    assert!(code <= 0x7F, "{key:?} has an out-of-range code {code:#x}");
                    if let Some(other) = codes.insert(code, key) {
                        panic!("{key:?} and {other:?} share the key code {code:#x}");
                    }
                }
                None => assert_eq!(key, Key::PrintScreen, "{key:?} is unmapped"),
            }
        }
        assert_eq!(codes.len(), Key::ALL.len() - 1);
    }

    #[test]
    fn key_codes_follow_the_us_ansi_layout() {
        // Spot checks against Events.h (kVK_ANSI_*, kVK_F*, arrows, specials).
        let expected = [
            (Key::A, 0x00),
            (Key::Z, 0x06),
            (Key::Q, 0x0C),
            (Key::M, 0x2E),
            (Key::Digit0, 0x1D),
            (Key::Digit4, 0x15),
            (Key::Digit5, 0x17),
            (Key::Digit6, 0x16),
            (Key::F1, 0x7A),
            (Key::F4, 0x76),
            (Key::F12, 0x6F),
            (Key::F20, 0x5A),
            (Key::Space, 0x31),
            (Key::Return, 0x24),
            (Key::Escape, 0x35),
            (Key::Backspace, 0x33),
            (Key::Delete, 0x75),
            (Key::Left, 0x7B),
            (Key::Up, 0x7E),
            (Key::Grave, 0x32),
        ];
        for (key, code) in expected {
            assert_eq!(key_code(key), Some(code), "{key:?}");
        }
    }

    #[test]
    fn modifiers_map_to_carbon_masks() {
        assert_eq!(modifier_mask(Modifiers::NONE), 0);
        assert_eq!(modifier_mask(Modifiers::CONTROL), ffi::controlKey);
        assert_eq!(modifier_mask(Modifiers::ALT), ffi::optionKey);
        assert_eq!(modifier_mask(Modifiers::SHIFT), ffi::shiftKey);
        assert_eq!(modifier_mask(Modifiers::SUPER), ffi::cmdKey);
        assert_eq!(
            modifier_mask(
                Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::SUPER
            ),
            0x1B00
        );
    }

    #[test]
    fn print_screen_is_unavailable() {
        let hotkey = Hotkey::new(Modifiers::SHIFT, Key::PrintScreen);
        assert!(matches!(
            carbon_combination(hotkey),
            Err(Error::HotkeyUnavailable { hotkey: failed, .. }) if failed == hotkey
        ));
        assert_eq!(
            carbon_combination(Hotkey::new(
                Modifiers::SUPER | Modifiers::SHIFT,
                Key::Digit4
            ))
            .unwrap(),
            (0x15, ffi::cmdKey | ffi::shiftKey)
        );
    }

    fn context(
        bindings: Vec<(u32, HotkeyBinding)>,
    ) -> (Context, event::EventReceiver<HotkeyEvent>) {
        let (sender, events) = event::channel();
        (Context { sender, bindings }, events)
    }

    fn binding(mode: CaptureMode, hotkey: Hotkey) -> HotkeyBinding {
        HotkeyBinding { mode, hotkey }
    }

    #[test]
    fn the_handler_claims_only_its_own_hotkey_ids() {
        let selection = binding(
            CaptureMode::Selection,
            Hotkey::new(Modifiers::SUPER | Modifiers::SHIFT, Key::F19),
        );
        let (context, events) = context(vec![(7, selection)]);
        let ours = EventHotKeyID {
            signature: SIGNATURE,
            id: 7,
        };
        assert!(!context.deliver(EventHotKeyID { id: 8, ..ours }));
        assert!(!context.deliver(EventHotKeyID {
            signature: u32::from_be_bytes(*b"Othr"),
            ..ours
        }));
        assert_eq!(events.try_recv(), None);
        assert!(context.deliver(ours));
        assert_eq!(
            events.try_recv(),
            Some(HotkeyEvent {
                mode: CaptureMode::Selection,
                hotkey: selection.hotkey
            })
        );
    }

    // The tests below use the real Carbon API. It is not thread-safe, and the
    // test harness runs tests on parallel threads, so they take turns.
    static CARBON: Mutex<()> = Mutex::new(());

    /// Test-only Carbon calls, to dispatch a synthetic hotkey press.
    #[allow(non_snake_case)]
    mod dispatch {
        use super::ffi::{EventRef, EventTargetRef, OSStatus, OSType};

        #[link(name = "Carbon", kind = "framework")]
        unsafe extern "C" {
            pub fn CreateEvent(
                inAllocator: *const std::ffi::c_void,
                inClassID: OSType,
                inKind: u32,
                inWhen: f64,
                inAttributes: u32,
                outEvent: *mut EventRef,
            ) -> OSStatus;
            pub fn SetEventParameter(
                inEvent: EventRef,
                inName: OSType,
                inType: OSType,
                inSize: usize,
                inDataPtr: *const std::ffi::c_void,
            ) -> OSStatus;
            pub fn SendEventToEventTarget(inEvent: EventRef, inTarget: EventTargetRef) -> OSStatus;
            pub fn ReleaseEvent(inEvent: EventRef);
        }
    }

    /// Dispatches a `kEventHotKeyPressed` for hotkey `id` to the application
    /// target, as Carbon does for a real press. Returns the handlers' verdict.
    fn press(id: u32) -> OSStatus {
        let hotkey_id = EventHotKeyID {
            signature: SIGNATURE,
            id,
        };
        let mut event: EventRef = ptr::null_mut();
        // SAFETY: Standard create/set/send/release sequence on a fresh event; the
        // parameter data is a live `EventHotKeyID` of the size passed in.
        unsafe {
            let status = dispatch::CreateEvent(
                ptr::null(),
                ffi::kEventClassKeyboard,
                ffi::kEventHotKeyPressed,
                0.0,
                0,
                &raw mut event,
            );
            assert_eq!(status, ffi::noErr, "CreateEvent");
            let status = dispatch::SetEventParameter(
                event,
                ffi::kEventParamDirectObject,
                ffi::typeEventHotKeyID,
                size_of::<EventHotKeyID>(),
                (&raw const hotkey_id).cast(),
            );
            assert_eq!(status, ffi::noErr, "SetEventParameter");
            let status = dispatch::SendEventToEventTarget(event, ffi::GetApplicationEventTarget());
            dispatch::ReleaseEvent(event);
            status
        }
    }

    fn register(
        bindings: &[HotkeyBinding],
    ) -> (
        Registered,
        Vec<(HotkeyBinding, Error)>,
        event::EventReceiver<HotkeyEvent>,
    ) {
        let (sender, events) = event::channel();
        let (registered, failures) =
            Registered::new(bindings, sender).expect("Carbon is available");
        (registered, failures, events)
    }

    fn id_of(registered: &Registered, binding: HotkeyBinding) -> u32 {
        // SAFETY: The context is live while `registered` is.
        let context = unsafe { &*registered.context };
        context
            .bindings
            .iter()
            .find(|(_, ours)| *ours == binding)
            .map(|&(id, _)| id)
            .expect("registered")
    }

    /// Combinations nothing on a development machine should use: F13–F20 with
    /// three or four modifiers.
    fn unlikely_hotkeys() -> Vec<Hotkey> {
        let (control, alt, shift, command) = (
            Modifiers::CONTROL,
            Modifiers::ALT,
            Modifiers::SHIFT,
            Modifiers::SUPER,
        );
        let modifier_sets = [
            control | alt | shift | command,
            control | alt | command,
            control | shift | command,
            alt | shift | command,
        ];
        let keys = [
            Key::F13,
            Key::F14,
            Key::F15,
            Key::F16,
            Key::F17,
            Key::F18,
            Key::F19,
            Key::F20,
        ];
        modifier_sets
            .into_iter()
            .flat_map(|modifiers| keys.map(|key| Hotkey::new(modifiers, key)))
            .collect()
    }

    /// Whether `error` is Carbon's `eventHotKeyExistsErr` for `hotkey`: another
    /// process or another registration in this one holds it.
    fn held_elsewhere(error: &Error, hotkey: Hotkey) -> bool {
        error.to_string() == registration_error(hotkey, ffi::eventHotKeyExistsErr).to_string()
    }

    /// A combination that a scenario had to register was held by something else.
    struct Held {
        hotkey: Hotkey,
        step: &'static str,
    }

    /// Checks that a registration the scenario relies on succeeded. A failure
    /// because the combination is held elsewhere is `Held`, so the scenario can be
    /// retried with other combinations; any other failure fails the test.
    fn needed(failures: &[(HotkeyBinding, Error)], step: &'static str) -> Result<(), Held> {
        match failures {
            [] => Ok(()),
            [(failed, error)] if held_elsewhere(error, failed.hotkey) => Err(Held {
                hotkey: failed.hotkey,
                step,
            }),
            _ => panic!("{step}: {failures:?}"),
        }
    }

    /// Runs `scenario` with `count` distinct combinations from
    /// [`unlikely_hotkeys`]. Registrations are global to the login session, and
    /// other processes (the tests of another checkout, a developer's app) may
    /// hold any combination exclusively. So each process starts at its own place
    /// in the pool, and a scenario that finds a combination held is run again
    /// with the next ones. Fails if every attempt found one held.
    fn with_free_hotkeys(count: usize, scenario: impl Fn(&[Hotkey]) -> Result<(), Held>) {
        let pool = unlikely_hotkeys();
        let start = (std::process::id() as usize).wrapping_mul(0x9E37_79B9) % pool.len();
        let mut held = Vec::new();
        for attempt in 0..pool.len() / count {
            let hotkeys: Vec<Hotkey> = (0..count)
                .map(|i| pool[(start + attempt * count + i) % pool.len()])
                .collect();
            match scenario(&hotkeys) {
                Ok(()) => return,
                Err(Held { hotkey, step }) => held.push(format!("{hotkey} at {step}")),
            }
        }
        panic!(
            "no attempt found its combinations free; each was held by another process or \
             never released: {}",
            held.join(", ")
        );
    }

    #[test]
    fn a_combination_is_free_again_once_its_registration_is_dropped() {
        let _carbon = CARBON.lock();
        with_free_hotkeys(1, |hotkeys| {
            let display = binding(CaptureMode::Display, hotkeys[0]);

            let (first, failures, _events) = register(&[display]);
            needed(&failures, "first registration")?;
            let (clash, failures, _clash_events) = register(&[display]);
            assert!(
                matches!(
                    failures.as_slice(),
                    [(failed, error)] if *failed == display && held_elsewhere(error, display.hotkey)
                ),
                "{failures:?}"
            );

            drop((first, clash));
            let (_second, failures, _events) = register(&[display]);
            needed(&failures, "registration after dropping the first")
        });
    }

    #[test]
    fn unsupported_keys_fail_alone() {
        let _carbon = CARBON.lock();
        with_free_hotkeys(1, |hotkeys| {
            let selection = binding(CaptureMode::Selection, hotkeys[0]);
            let screen = binding(
                CaptureMode::Display,
                Hotkey::new(Modifiers::SHIFT, Key::PrintScreen),
            );
            let (registered, failures, _events) = register(&[screen, selection]);
            let (unsupported, others): (Vec<_>, Vec<_>) = failures
                .into_iter()
                .partition(|(failed, _)| *failed == screen);
            assert!(
                matches!(
                    unsupported.as_slice(),
                    [(_, Error::HotkeyUnavailable { hotkey, .. })] if *hotkey == screen.hotkey
                ),
                "{unsupported:?}"
            );
            needed(&others, "registering the supported key")?;
            assert_eq!(registered.hotkeys.len(), 1);
            id_of(&registered, selection);
            Ok(())
        });
    }

    #[test]
    fn presses_reach_the_registration_that_owns_the_hotkey() {
        let _carbon = CARBON.lock();
        with_free_hotkeys(2, |hotkeys| {
            let display = binding(CaptureMode::Display, hotkeys[0]);
            let selection = binding(CaptureMode::Selection, hotkeys[1]);
            let (first, failures, first_events) = register(&[display]);
            needed(&failures, "registering the first set")?;
            let (second, failures, second_events) = register(&[selection]);
            needed(&failures, "registering the second set")?;
            let (display_id, selection_id) = (id_of(&first, display), id_of(&second, selection));

            assert_eq!(press(selection_id), ffi::noErr);
            assert_eq!(first_events.try_recv(), None);
            assert_eq!(
                second_events.try_recv(),
                Some(HotkeyEvent {
                    mode: CaptureMode::Selection,
                    hotkey: selection.hotkey
                })
            );
            assert_eq!(press(display_id), ffi::noErr);
            assert_eq!(
                first_events.try_recv().map(|event| event.mode),
                Some(CaptureMode::Display)
            );

            // Once dropped, nobody claims the old id.
            drop(first);
            assert_eq!(press(display_id), ffi::eventNotHandledErr);
            assert_eq!(second_events.try_recv(), None);
            Ok(())
        });
    }
}
