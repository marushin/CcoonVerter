//! Native message boxes (Python `tkinter.messagebox` equivalents) via `rfd`.
//! All calls block and must be made from the UI thread.

use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

use crate::controller::MessageKind;

fn level(kind: MessageKind) -> MessageLevel {
    match kind {
        MessageKind::Info => MessageLevel::Info,
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

/// Show a message with an OK button.
pub fn show(parent: Option<&eframe::Frame>, kind: MessageKind, title: &str, body: &str) {
    dialog(parent, kind, title, body)
        .set_buttons(MessageButtons::Ok)
        .show();
}

pub fn info(parent: Option<&eframe::Frame>, title: &str, body: &str) {
    show(parent, MessageKind::Info, title, body);
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
    matches!(
        dialog(parent, kind, title, body)
            .set_buttons(MessageButtons::YesNo)
            .show(),
        MessageDialogResult::Yes
    )
}

/// Python `messagebox.askyesno` (question icon ≈ info level).
pub fn confirm(parent: Option<&eframe::Frame>, title: &str, body: &str) -> bool {
    ask_yes_no(parent, MessageKind::Warning, title, body)
}
