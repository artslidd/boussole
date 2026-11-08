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
///
/// One `arboard::Clipboard` is kept for the life of the app: on Linux/X11 without a clipboard
/// manager the text is served by that instance and disappears when it is dropped, so creating a
/// new one for every copy would say "Copié" and paste nothing. It is created on the first copy
/// and replaced if a copy fails (the connection to the display server may have gone away).
#[derive(Default)]
pub struct SystemClipboard {
    instance: RefCell<Option<arboard::Clipboard>>,
}

impl Clipboard for SystemClipboard {
    fn set_text(&self, text: &str) -> Result<(), String> {
        let mut instance = self.instance.borrow_mut();
        let mut last_error = String::new();
        // A kept instance that went stale gets one fresh start.
        for _ in 0..2 {
            if instance.is_none() {
                match arboard::Clipboard::new() {
                    Ok(clipboard) => *instance = Some(clipboard),
                    Err(error) => return Err(error.to_string()),
                }
            }
            let Some(clipboard) = instance.as_mut() else {
                continue;
            };
            match clipboard.set_text(text.to_owned()) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    last_error = error.to_string();
                    *instance = None;
                }
            }
        }
        Err(last_error)
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
