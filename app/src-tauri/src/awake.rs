//! Keeps the Mac out of idle sleep and App Nap while a run is live, so a
//! hidden window cannot starve the run. Call `set` from the main thread only.

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    thread_local! {
        static ACTIVITY: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
            const { RefCell::new(None) };
    }

    pub fn set(live: bool) {
        ACTIVITY.with_borrow_mut(|activity| {
            let info = NSProcessInfo::processInfo();
            match (live, activity.take()) {
                (true, None) => {
                    *activity = Some(info.beginActivityWithOptions_reason(
                        NSActivityOptions::UserInitiated,
                        &NSString::from_str("A deployment is running"),
                    ));
                }
                (true, Some(token)) => *activity = Some(token),
                // SAFETY: the token came from beginActivityWithOptions_reason.
                (false, Some(token)) => unsafe { info.endActivity(&token) },
                (false, None) => {}
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use mac::set;

#[cfg(not(target_os = "macos"))]
pub fn set(_live: bool) {}

#[cfg(test)]
mod tests {
    #[test]
    fn activity_begins_once_and_ends() {
        super::set(true);
        super::set(true);
        super::set(false);
        super::set(false);
    }
}
