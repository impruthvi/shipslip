//! macOS asks the app delegate before a quit it starts itself: Dock Quit, log
//! out, another app asking Shipslip to quit. tao does not answer, so such a quit
//! ended the app with a run live or a preview holding the server lock. This
//! answers it: cancel, and let the app's own quit path ask or release first.

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::CStr;
    use std::sync::OnceLock;

    use objc2::ffi;
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::sel;
    use tauri::AppHandle;

    /// tao's app delegate class.
    const DELEGATE: &CStr = c"TaoAppDelegateParent";
    /// NSApplicationTerminateReply.
    pub(super) const CANCEL: usize = 0;
    pub(super) const NOW: usize = 1;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
        match APP.get() {
            Some(app) if crate::quit_needs_app(app) => {
                crate::request_quit(app);
                CANCEL
            }
            _ => NOW,
        }
    }

    /// Answers `applicationShouldTerminate:` for tao's delegate. Call once the
    /// event loop exists, from setup.
    pub fn install(app: &AppHandle) {
        let _ = APP.set(app.clone());
        add_to(DELEGATE);
    }

    pub(super) fn add_to(class: &CStr) -> bool {
        let Some(cls) = AnyClass::get(class) else {
            return false;
        };
        let reply: extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> usize =
            should_terminate;
        // SAFETY: the function matches the "Q@:@" encoding (NSUInteger, self,
        // _cmd, sender) and the class lives for the whole process.
        unsafe {
            let imp: Imp = std::mem::transmute(reply);
            ffi::class_addMethod(
                cls as *const AnyClass as *mut AnyClass,
                sel!(applicationShouldTerminate:),
                imp,
                c"Q@:@".as_ptr(),
            )
            .as_bool()
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::install;

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &tauri::AppHandle) {}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, NSObject};
    use objc2::{msg_send, ClassType};

    use super::mac::{add_to, NOW};

    #[test]
    fn a_quit_with_nothing_to_finish_goes_ahead() {
        let builder = ClassBuilder::new(c"ShipslipTerminateTest", NSObject::class()).unwrap();
        builder.register();
        assert!(add_to(c"ShipslipTerminateTest"));
        let class = AnyClass::get(c"ShipslipTerminateTest").unwrap();
        let delegate: *mut AnyObject = unsafe { msg_send![class, new] };
        let reply: usize = unsafe {
            msg_send![delegate, applicationShouldTerminate: std::ptr::null_mut::<AnyObject>()]
        };
        assert_eq!(reply, NOW);
        let () = unsafe { msg_send![delegate, release] };
    }

    #[test]
    fn a_missing_delegate_class_is_left_alone() {
        assert!(!add_to(c"ShipslipNoSuchDelegate"));
    }
}
