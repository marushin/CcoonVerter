//! Native message boxes (Python `tkinter.messagebox` equivalents) via `rfd`.
//! All calls block and must be made from the UI thread.

#[cfg(not(test))]
use rfd::MessageDialogResult;
use rfd::{MessageButtons, MessageDialog, MessageLevel};

use crate::controller::MessageKind;

fn level(kind: MessageKind) -> MessageLevel {
    match kind {
        MessageKind::Warning => MessageLevel::Warning,
        MessageKind::Error => MessageLevel::Error,
    }
}

fn dialog(
    parent: Option<&eframe::Frame>,
    kind: MessageKind,
    title: &str,
    body: &str,
) -> MessageDialog {
    let d = MessageDialog::new()
        .set_level(level(kind))
        .set_title(title)
        .set_description(body);
    // Parenting makes the box modal to our window on Windows. Kept Windows-only
    // because it is the only platform where it has been exercised.
    #[cfg(target_os = "windows")]
    let d = match parent {
        Some(p) => d.set_parent(p),
        None => d,
    };
    #[cfg(not(target_os = "windows"))]
    let _ = parent;
    d
}

#[cfg(test)]
thread_local! {
    /// Answer to yes/no questions in tests, where no native box may open.
    pub static TEST_ANSWER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Show a message with an OK button.
pub fn show(parent: Option<&eframe::Frame>, kind: MessageKind, title: &str, body: &str) {
    if cfg!(test) {
        return;
    }
    dialog(parent, kind, title, body)
        .set_buttons(MessageButtons::Ok)
        .show();
}

pub fn error(parent: Option<&eframe::Frame>, title: &str, body: &str) {
    show(parent, MessageKind::Error, title, body);
}

/// Yes/No question; true for Yes.
pub fn ask_yes_no(
    parent: Option<&eframe::Frame>,
    kind: MessageKind,
    title: &str,
    body: &str,
) -> bool {
    #[cfg(test)]
    let answer = {
        let _ = (parent, kind, title, body);
        TEST_ANSWER.with(|a| a.get())
    };
    #[cfg(not(test))]
    let answer = matches!(
        dialog(parent, kind, title, body)
            .set_buttons(MessageButtons::YesNo)
            .show(),
        MessageDialogResult::Yes
    );
    answer
}

/// Python `messagebox.askyesno` (question icon ≈ info level).
pub fn confirm(parent: Option<&eframe::Frame>, title: &str, body: &str) -> bool {
    ask_yes_no(parent, MessageKind::Warning, title, body)
}
