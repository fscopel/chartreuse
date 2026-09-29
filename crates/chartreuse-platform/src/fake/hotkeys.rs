//! Fake [`Hotkeys`]: registrations recorded in the world, pressed with
//! [`Fake::press_hotkey`].

use std::sync::{Arc, Weak};

use chartreuse_core::{Error, Result};
use parking_lot::Mutex;

use super::{Fake, State};
use crate::event::{self, Registration};
use crate::hotkeys::{HotkeyBinding, HotkeyRegistration, Hotkeys};

/// Removes one registration from the world when dropped.
struct Unregister {
    state: Weak<Mutex<State>>,
    generation: u64,
}

impl Drop for Unregister {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state
                .lock()
                .hotkeys
                .retain(|(generation, _, _)| *generation != self.generation);
        }
    }
}

impl Hotkeys for Fake {
    fn register(&self, bindings: &[HotkeyBinding]) -> Result<HotkeyRegistration> {
        let mut state = self.state.lock();
        let (sender, events) = event::channel();
        let mut active: Vec<HotkeyBinding> = Vec::new();
        let mut failures = Vec::new();
        for &binding in bindings {
            let taken_elsewhere = state.reserved_hotkeys.contains(&binding.hotkey);
            let taken_by_us = state
                .hotkeys
                .iter()
                .flat_map(|(_, bindings, _)| bindings)
                .chain(&active)
                .any(|other| other.hotkey == binding.hotkey);
            if taken_elsewhere || taken_by_us {
                let reason = if taken_elsewhere {
                    "another program has registered it"
                } else {
                    "it is already registered"
                };
                failures.push((
                    binding,
                    Error::HotkeyUnavailable {
                        hotkey: binding.hotkey,
                        reason: reason.into(),
                    },
                ));
            } else {
                active.push(binding);
            }
        }
        let generation = state.next_generation();
        state.hotkeys.push((generation, active, sender));
        Ok(HotkeyRegistration {
            events,
            failures,
            registration: Registration::new(Unregister {
                state: Arc::downgrade(&self.state),
                generation,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::capture::CaptureMode;
    use chartreuse_core::hotkey::{Hotkey, Key, Modifiers};

    use super::*;
    use crate::hotkeys::HotkeyEvent;

    fn binding(mode: CaptureMode, key: Key) -> HotkeyBinding {
        HotkeyBinding {
            mode,
            hotkey: Hotkey::new(Modifiers::SUPER | Modifiers::SHIFT, key),
        }
    }

    #[test]
    fn presses_of_registered_hotkeys_are_delivered() {
        let fake = Fake::new();
        let display = binding(CaptureMode::Display, Key::Digit1);
        let registration = fake.register(&[display]).unwrap();
        assert!(registration.failures.is_empty());
        assert!(fake.press_hotkey(display.hotkey));
        assert!(!fake.press_hotkey(binding(CaptureMode::Display, Key::Digit2).hotkey));
        assert_eq!(
            registration.events.try_recv(),
            Some(HotkeyEvent {
                mode: CaptureMode::Display,
                hotkey: display.hotkey
            })
        );
        assert_eq!(registration.events.try_recv(), None);
    }

    #[test]
    fn conflicts_are_reported_per_binding_and_the_rest_stay_active() {
        let fake = Fake::new();
        let display = binding(CaptureMode::Display, Key::Digit2);
        let selection = binding(CaptureMode::Selection, Key::Digit3);
        fake.reserve_hotkey(display.hotkey);
        let registration = fake.register(&[display, selection]).unwrap();
        assert_eq!(registration.failures.len(), 1);
        assert!(matches!(
            registration.failures[0],
            (failed, Error::HotkeyUnavailable { hotkey, .. }) if failed == display && hotkey == display.hotkey
        ));
        assert_eq!(fake.registered_hotkeys(), [selection]);
    }

    #[test]
    fn re_registering_requires_dropping_the_previous_registration() {
        let fake = Fake::new();
        let display = binding(CaptureMode::Display, Key::Digit1);
        let first = fake.register(&[display]).unwrap();
        let clash = fake.register(&[display]).unwrap();
        assert_eq!(
            clash.failures.len(),
            1,
            "combination still held by the first registration"
        );
        drop((first, clash));
        assert!(fake.registered_hotkeys().is_empty());
        assert!(!fake.press_hotkey(display.hotkey));
        let second = fake.register(&[display]).unwrap();
        assert!(second.failures.is_empty());
        assert!(fake.press_hotkey(display.hotkey));
        assert!(second.events.try_recv().is_some());
    }
}
