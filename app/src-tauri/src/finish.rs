//! What a run that ends while the window is away leaves outside it: a Dock
//! bounce, the window title, and a "!" badge until that run's receipt is seen.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use tauri::{AppHandle, Manager, UserAttentionType, WebviewWindow};

use crate::commands;
use crate::deploy_session::Ending;

pub const TITLE: &str = "Shipslip";

/// Runs whose receipt has not been seen since they ended flagged.
#[derive(Default)]
pub struct Unseen(Mutex<BTreeSet<String>>);

impl Unseen {
    /// Returns whether the badge shows. Only a flagged run that ended while
    /// the window was away is kept: otherwise its end state was on screen.
    pub fn finished(&self, ending: &Ending, away: bool) -> bool {
        let mut runs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let (true, true, Some(run_id)) = (away, ending.flagged, &ending.run_id) {
            runs.insert(run_id.clone());
        }
        !runs.is_empty()
    }

    /// The pane shows `run_id`'s receipt. It counts as seen only while the
    /// window is focused. Returns the badge state when it changed.
    pub fn seen(&self, run_id: &str, focused: bool) -> Option<bool> {
        let mut runs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (focused && runs.remove(run_id)).then(|| !runs.is_empty())
    }
}

pub fn focused(window: &WebviewWindow) -> bool {
    window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
}

pub fn badge(window: &WebviewWindow, show: bool) {
    #[cfg(target_os = "macos")]
    let _ = window.set_badge_label(show.then(|| "!".into()));
    #[cfg(not(target_os = "macos"))]
    let _ = (window, show);
}

/// Runs on the main thread, in the same order as `receipt_seen`.
pub fn finished(app: &AppHandle, ending: &Ending) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let away = !focused(&window);
    if away {
        let _ = window.request_user_attention(Some(if ending.critical {
            UserAttentionType::Critical
        } else {
            UserAttentionType::Informational
        }));
        let _ = window.set_title(&ending.title);
    }
    let show = app
        .state::<Arc<commands::App>>()
        .unseen()
        .finished(ending, away);
    badge(&window, show);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ending(run_id: &str, flagged: bool) -> Ending {
        Ending {
            run_id: Some(run_id.into()),
            title: "shop → production: failed at step 2".into(),
            critical: flagged,
            flagged,
        }
    }

    #[test]
    fn a_flagged_run_that_ends_away_is_badged_until_its_receipt_is_seen_focused() {
        let unseen = Unseen::default();
        assert!(unseen.finished(&ending("run-1", true), true));
        assert_eq!(unseen.seen("run-1", false), None, "shown while unfocused");
        assert_eq!(unseen.seen("run-2", true), None, "another receipt shown");
        assert_eq!(unseen.seen("run-1", true), Some(false));
        assert_eq!(unseen.seen("run-1", true), None);
    }

    #[test]
    fn the_badge_stays_until_every_flagged_run_is_seen() {
        let unseen = Unseen::default();
        unseen.finished(&ending("run-1", true), true);
        assert!(unseen.finished(&ending("run-2", true), true));
        assert_eq!(unseen.seen("run-2", true), Some(true));
        assert_eq!(unseen.seen("run-1", true), Some(false));
    }

    #[test]
    fn an_unflagged_end_or_one_shown_in_the_window_is_not_badged() {
        let unseen = Unseen::default();
        assert!(!unseen.finished(&ending("run-1", false), true));
        assert!(!unseen.finished(&ending("run-2", true), false));
        let unrecorded = Ending {
            run_id: None,
            ..ending("run-3", true)
        };
        assert!(!unseen.finished(&unrecorded, true));
        assert!(unseen.finished(&ending("run-4", true), true));
        assert!(unseen.finished(&ending("run-5", false), true), "badge kept");
    }
}
