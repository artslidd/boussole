//! Copying text to the system clipboard (room code, address to share).
//!
//! Slint offers applications no clipboard API, so the app uses `arboard`. The trait keeps the UI
//! testable: tests use [`MemoryClipboard`].

use std::cell::RefCell;

/// Somewhere to copy text to.
pub trait Clipboard {
    /// Replace the clipboard content with `text`. `Err` carries the technical reason (no
    /// clipboard available, for instance on a headless machine).
    fn set_text(&self, text: &str) -> Result<(), String>;
}

/// The operating system's clipboard.
#[derive(Debug, Default)]
pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn set_text(&self, text: &str) -> Result<(), String> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text.to_owned()))
            .map_err(|error| error.to_string())
    }
}

/// A clipboard that remembers what was copied (tests, screenshots); can be made to fail.
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    content: RefCell<Option<String>>,
    fail: std::cell::Cell<bool>,
}

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// What was copied last.
    pub fn content(&self) -> Option<String> {
        self.content.borrow().clone()
    }

    /// Make every following copy fail (no clipboard on this machine).
    pub fn set_failing(&self, fail: bool) {
        self.fail.set(fail);
    }
}

impl Clipboard for MemoryClipboard {
    fn set_text(&self, text: &str) -> Result<(), String> {
        if self.fail.get() {
            return Err("pas de presse-papiers".to_owned());
        }
        *self.content.borrow_mut() = Some(text.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_clipboard_keeps_the_last_text_and_can_fail() {
        let clipboard = MemoryClipboard::new();
        assert_eq!(clipboard.content(), None);
        clipboard.set_text("K7QX2").unwrap();
        clipboard.set_text("wss://x.trycloudflare.com/ws").unwrap();
        assert_eq!(
            clipboard.content().as_deref(),
            Some("wss://x.trycloudflare.com/ws")
        );
        clipboard.set_failing(true);
        assert!(clipboard.set_text("nope").is_err());
        assert_eq!(
            clipboard.content().as_deref(),
            Some("wss://x.trycloudflare.com/ws")
        );
    }
}
