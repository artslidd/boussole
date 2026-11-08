//! A window without a screen: the software renderer drawing into memory.
//!
//! Used by `examples/screenshots.rs` (PNG screenshots for the README) and by the UI tests
//! (`tests/ui_flow.rs`) to drive the real `.slint` UI with keyboard and mouse events and to
//! render it. Slint keeps its platform per thread, so call [`Headless::new`] once per thread
//! (each `#[test]` runs on its own thread).

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, PhysicalSize, PlatformError as SlintPlatformError, SharedString};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

struct HeadlessPlatform {
    window: Rc<MinimalSoftwareWindow>,
    clock: Rc<Cell<Duration>>,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.clock.get()
    }
}

/// An in-memory window plus a mock clock, to drive and render a Slint component.
pub struct Headless {
    window: Rc<MinimalSoftwareWindow>,
    clock: Rc<Cell<Duration>>,
}

impl Headless {
    /// Install the headless platform on the current thread. `width × height` are logical pixels
    /// and `scale` the device scale factor (2.0 for retina-sharp screenshots).
    ///
    /// Must be called before any Slint component is created on this thread.
    pub fn new(width: u32, height: u32, scale: f32) -> Result<Self, SlintPlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(HeadlessPlatform {
            window: window.clone(),
            clock: clock.clone(),
        }))
        .map_err(|e| SlintPlatformError::Other(format!("cannot install headless platform: {e}")))?;
        let headless = Self { window, clock };
        headless.resize(width, height, scale);
        Ok(headless)
    }

    /// Resize the window (logical pixels) and set the scale factor.
    pub fn resize(&self, width: u32, height: u32, scale: f32) {
        self.window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        self.window.set_size(PhysicalSize::new(
            (width as f32 * scale).round() as u32,
            (height as f32 * scale).round() as u32,
        ));
        self.window.dispatch_event(WindowEvent::Resized {
            size: slint::LogicalSize::new(width as f32, height as f32),
        });
    }

    /// Run timers, change handlers and a layout/render pass until the UI is stable.
    pub fn settle(&self) {
        let mut scratch = Vec::new();
        for _ in 0..4 {
            slint::platform::update_timers_and_animations();
            let size = self.window.size();
            let needed = size.width as usize * size.height as usize;
            scratch.resize(
                needed,
                slint::platform::software_renderer::PremultipliedRgbaColor::default(),
            );
            self.window.draw_if_needed(|renderer| {
                renderer.render(&mut scratch, size.width as usize);
            });
        }
    }

    /// Let `duration` of animation time pass, then settle.
    pub fn advance(&self, duration: Duration) {
        self.clock.set(self.clock.get() + duration);
        self.settle();
    }

    /// Render the whole window into an RGBA pixel buffer.
    pub fn snapshot(&self, component: &impl ComponentHandle) -> SharedPixelBuffer<Rgba8Pixel> {
        self.settle();
        component
            .window()
            .take_snapshot()
            .expect("the software renderer can always take a snapshot")
    }

    /// Press and release a key. `text` is the typed character, or a [`slint::platform::Key`]
    /// converted to text for special keys (`Key::DownArrow.into()`).
    pub fn press_key(&self, text: impl Into<SharedString>) {
        let text = text.into();
        self.window
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window
            .dispatch_event(WindowEvent::KeyReleased { text });
        self.settle();
    }

    /// Send an auto-repeat of a held key (the operating system keeps sending the key press).
    pub fn press_key_repeat(&self, text: impl Into<SharedString>) {
        self.window
            .dispatch_event(WindowEvent::KeyPressRepeated { text: text.into() });
        self.settle();
    }

    /// Type a string character by character.
    pub fn type_text(&self, text: &str) {
        for ch in text.chars() {
            self.press_key(ch.to_string());
        }
    }

    /// Turn the mouse wheel at a position (negative `delta_y` scrolls the content down).
    pub fn scroll(&self, x: f32, y: f32, delta_y: f32) {
        let position = slint::LogicalPosition::new(x, y);
        self.window
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.window.dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y,
        });
        self.settle();
    }

    /// Click at a position in logical pixels.
    pub fn click(&self, x: f32, y: f32) {
        let position = slint::LogicalPosition::new(x, y);
        self.window
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
        self.window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
        self.settle();
    }
}
