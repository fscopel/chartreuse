//! Fake [`LaunchAtLogin`]: an in-memory login item, which tests can change
//! behind the app's back, or make refuse changes or fail to read.

use chartreuse_core::Result;

use super::Fake;
use crate::launch_at_login::LaunchAtLogin;

impl LaunchAtLogin for Fake {
    fn status(&self) -> Result<bool> {
        let state = self.state.lock();
        match &state.launch_at_login_unreadable {
            Some(error) => Err(error.clone()),
            None => Ok(state.launch_at_login),
        }
    }

    fn set(&self, enabled: bool) -> Result<()> {
        let mut state = self.state.lock();
        if let Some(error) = &state.launch_at_login_refusal {
            return Err(error.clone());
        }
        state.launch_at_login = enabled;
        state.launch_at_login_sets.push(enabled);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chartreuse_core::Error;

    use super::*;

    #[test]
    fn a_refused_change_changes_nothing() {
        let fake = Fake::new();
        fake.set_launch_at_login_refusal(Some(Error::Platform("denied".into())));
        assert!(fake.set(true).is_err());
        assert!(!fake.status().unwrap());
        assert!(fake.launch_at_login_sets().is_empty());

        fake.set_launch_at_login_refusal(None);
        fake.set(true).unwrap();
        assert!(fake.status().unwrap());
        assert_eq!(fake.launch_at_login_sets(), [true]);
    }
}
