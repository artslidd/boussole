//! ⌘Q and the application menu's *Quit* (macOS).
//!
//! Slint's macOS backend always installs its default application menu, whose *Quit* item sends
//! AppKit's `terminate:`; AppKit then ends the process with `exit(0)` without ever asking the
//! window to close. That would skip the "Quitter fermera la partie pour tout le monde"
//! confirmation, `leave_room`, and the stop of the embedded server and its `cloudflared` tunnel.
//! (A Slint `MenuBar` does not help in 1.18: on macOS the default application menu is created
//! next to it anyway, and without native menu support a `MenuBar` would be drawn inside every
//! window.) So the implementation of `-[NSApplication terminate:]` is replaced by one that calls a
//! handler, which routes the request through the same path as closing the window.
//!
//! Dock → Quit and the system's quit request end up in `terminate:` too. Elsewhere (Windows,
//! Linux) the application is quit by closing its window, which already takes that path, and
//! everything here is a no-op.

/// Replace `terminate:` by a call to `handler` (made on the main thread, inside AppKit's event
/// handling: hand the work over to the event loop and return at once). Installed at most once
/// per process; `Err` explains why it could not be.
#[cfg(target_os = "macos")]
pub fn intercept_terminate(handler: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
    imp::intercept_terminate(Box::new(handler))
}

/// What `terminate:` would have done: really quit the process. A fallback when the handler has
/// nowhere to send the request.
#[cfg(target_os = "macos")]
pub fn terminate_now() {
    imp::terminate_now();
}

/// Send `terminate:` to the application like the *Quit* menu item does (tests, and the
/// real-window example).
#[cfg(target_os = "macos")]
pub fn send_terminate() {
    imp::send_terminate();
}

#[cfg(not(target_os = "macos"))]
pub fn intercept_terminate(_handler: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn terminate_now() {}

#[cfg(not(target_os = "macos"))]
pub fn send_terminate() {}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::OnceLock;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn class_getInstanceMethod(class: *mut c_void, sel: *mut c_void) -> *mut c_void;
        fn method_setImplementation(method: *mut c_void, imp: *const c_void) -> *const c_void;
        /// Never called as declared: cast to the exact signature of each message (the variadic
        /// calling convention differs from the fixed one on Apple silicon).
        fn objc_msgSend();
    }

    type Handler = Box<dyn Fn() + Send + Sync>;
    static HANDLER: OnceLock<Handler> = OnceLock::new();
    /// The original `terminate:` implementation, as an address.
    static ORIGINAL: OnceLock<usize> = OnceLock::new();

    fn sel(name: &CStr) -> *mut c_void {
        // SAFETY: `name` is a valid NUL-terminated string.
        unsafe { sel_registerName(name.as_ptr()) }
    }

    fn shared_application() -> *mut c_void {
        // SAFETY: `+[NSApplication sharedApplication]` takes no argument and returns an object;
        // `objc_msgSend` is cast to that signature.
        unsafe {
            let class = objc_getClass(c"NSApplication".as_ptr());
            if class.is_null() {
                return std::ptr::null_mut();
            }
            let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as *const c_void);
            send(class, sel(c"sharedApplication"))
        }
    }

    /// The replacement `-[NSApplication terminate:]`.
    extern "C" fn terminate(_this: *mut c_void, _cmd: *mut c_void, _sender: *mut c_void) {
        match HANDLER.get() {
            Some(handler) => handler(),
            None => terminate_now(),
        }
    }

    pub fn intercept_terminate(handler: Handler) -> Result<(), String> {
        if HANDLER.set(handler).is_err() {
            return Err("terminate: is already intercepted".to_owned());
        }
        // SAFETY: plain Objective-C runtime calls on the `NSApplication` class; the replacement
        // has the signature of `terminate:` (receiver, selector, sender).
        unsafe {
            let class = objc_getClass(c"NSApplication".as_ptr());
            if class.is_null() {
                return Err("NSApplication is not available".to_owned());
            }
            let method = class_getInstanceMethod(class, sel(c"terminate:"));
            if method.is_null() {
                return Err("NSApplication has no terminate:".to_owned());
            }
            let previous = method_setImplementation(method, terminate as *const c_void);
            let _ = ORIGINAL.set(previous as usize);
        }
        Ok(())
    }

    pub fn terminate_now() {
        let Some(&original) = ORIGINAL.get() else {
            // Not intercepted: `terminate:` is the original, send it.
            send_terminate();
            return;
        };
        let app = shared_application();
        if app.is_null() {
            return;
        }
        // SAFETY: `original` is the implementation `method_setImplementation` returned for
        // `terminate:`, which has this signature.
        unsafe {
            let original: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
                std::mem::transmute(original);
            original(app, sel(c"terminate:"), std::ptr::null_mut());
        }
    }

    pub fn send_terminate() {
        let app = shared_application();
        if app.is_null() {
            return;
        }
        // SAFETY: `-terminate:` takes one object argument (the sender); `objc_msgSend` is cast to
        // that signature.
        unsafe {
            let send: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
                std::mem::transmute(objc_msgSend as *const c_void);
            send(app, sel(c"terminate:"), std::ptr::null_mut());
        }
    }
}
