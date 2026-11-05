//! Update check (P10) as `impl App`: the daemon's state, the dismissible banner and the manual check.

use super::*;

use as_core::updates::{CheckStatus, UpdateInfo, UpdateState};

impl App {
    /// The newer release to announce in the banner (none once dismissed for that version).
    pub fn update_banner(&self) -> Option<&UpdateInfo> {
        let info = self.update.as_ref()?.available()?;
        (self.update_dismissed.as_deref() != Some(info.version.as_str())).then_some(info)
    }

    /// How to upgrade, for the banner and the toast.
    pub fn update_hint(&self) -> &str {
        self.update.as_ref().map_or("", |u| u.hint.as_str())
    }

    pub(super) fn set_update_state(&mut self, state: UpdateState) {
        self.settings.update = Some(state.clone());
        self.update = Some(state);
        self.dirty = true;
    }

    /// `Event::UpdateAvailable`: a newer version was found by the daemon's check.
    pub(super) fn on_update_available(&mut self, state: UpdateState) {
        self.set_update_state(state);
    }

    /// Esc (`close-dialogs`) also dismisses the banner, for that version only.
    pub(super) fn dismiss_update_banner(&mut self) {
        if let Some(info) = self.update_banner() {
            self.update_dismissed = Some(info.version.clone());
        }
    }

    /// `Command::CheckUpdates`: ask the daemon now; the answer is a toast.
    pub(super) fn check_updates(&mut self) -> Vec<Effect> {
        self.toast(ToastKind::Info, "Updates", "Checking for updates\u{2026}");
        vec![crate::panels::req(
            "upd:check",
            as_proto::Request::CheckForUpdates,
        )]
    }

    pub(super) fn on_update_reply(
        &mut self,
        key: &str,
        result: Result<as_proto::Response, String>,
    ) {
        match result {
            Ok(as_proto::Response::UpdateState(state)) => {
                let state = *state;
                if key == "upd:check" {
                    let (kind, body) = match state.status {
                        CheckStatus::Available => {
                            // A manual check shows the banner again.
                            self.update_dismissed = None;
                            (
                                ToastKind::Success,
                                format!("{}. {}", state.summary(), state.hint),
                            )
                        }
                        CheckStatus::UpToDate | CheckStatus::NoReleases | CheckStatus::Idle => {
                            (ToastKind::Info, state.summary())
                        }
                        _ => (ToastKind::Warn, state.summary()),
                    };
                    self.toast(kind, "Updates", &body);
                }
                self.set_update_state(state);
            }
            Err(e) if key == "upd:check" => self.toast(ToastKind::Error, "Updates", &e),
            // An older daemon does not know the request: no banner, nothing to say.
            _ => {}
        }
    }
}
