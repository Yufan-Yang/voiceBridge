//! Clipboard preservation around a paste.

use crate::error::Result;
use crate::platform::ClipboardManager;

/// Remembers the user's clipboard, replaces it with the text to inject, and
/// later restores it — unless the user changed the clipboard in between.
pub struct ClipboardSession<'a> {
    clipboard: &'a dyn ClipboardManager,
    saved: Option<String>,
    /// Clipboard version right after our own write.
    our_version: i64,
}

impl<'a> ClipboardSession<'a> {
    pub fn begin(clipboard: &'a dyn ClipboardManager, text: &str) -> Result<Self> {
        let saved = clipboard.read_text()?;
        clipboard.write_text(text)?;
        let our_version = clipboard.change_count()?;
        Ok(Self {
            clipboard,
            saved,
            our_version,
        })
    }

    /// Restores the previous contents. Returns false (and leaves the
    /// clipboard alone) when someone else wrote to it after us.
    pub fn restore(self) -> Result<bool> {
        if self.clipboard.change_count()? != self.our_version {
            return Ok(false);
        }
        match &self.saved {
            Some(text) => self.clipboard.write_text(text)?,
            None => self.clipboard.clear()?,
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::MockDesktop;

    #[test]
    fn restores_previous_contents() {
        let desk = MockDesktop::new();
        desk.write_text("user data").unwrap();
        let session = ClipboardSession::begin(&desk, "injected").unwrap();
        assert_eq!(desk.read_text().unwrap().as_deref(), Some("injected"));
        assert!(session.restore().unwrap());
        assert_eq!(desk.read_text().unwrap().as_deref(), Some("user data"));
    }

    #[test]
    fn empty_clipboard_is_cleared_again() {
        let desk = MockDesktop::new();
        let session = ClipboardSession::begin(&desk, "injected").unwrap();
        assert!(session.restore().unwrap());
        assert_eq!(desk.read_text().unwrap(), None);
    }

    #[test]
    fn user_change_is_never_overwritten() {
        let desk = MockDesktop::new();
        desk.write_text("old").unwrap();
        let session = ClipboardSession::begin(&desk, "injected").unwrap();
        desk.write_text("copied by the user meanwhile").unwrap();
        assert!(!session.restore().unwrap());
        assert_eq!(
            desk.read_text().unwrap().as_deref(),
            Some("copied by the user meanwhile")
        );
    }
}
