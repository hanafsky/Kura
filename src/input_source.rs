//! Switch the macOS keyboard input source (e.g. to ASCII "英数" on startup).

/// Input source selected when kura starts.
pub const ASCII_INPUT_SOURCE: &str = "com.apple.keylayout.ABC";

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_void, CString};
    use std::os::raw::{c_char, c_long};

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type CFIndex = c_long;
    type TISInputSourceRef = *const c_void;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        fn CFStringCreateWithCString(
            alloc: CFAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFDictionaryCreate(
            alloc: CFAllocatorRef,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: CFIndex,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;
        fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: CFIndex) -> *const c_void;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        static kTISPropertyInputSourceID: CFStringRef;
        fn TISCreateInputSourceList(
            properties: CFDictionaryRef,
            include_all_installed: u8,
        ) -> CFArrayRef;
        fn TISSelectInputSource(source: TISInputSourceRef) -> i32;
    }

    pub fn select(id: &str) -> bool {
        let Ok(c_id) = CString::new(id) else {
            return false;
        };
        unsafe {
            let cf_id = CFStringCreateWithCString(
                std::ptr::null(),
                c_id.as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            );
            if cf_id.is_null() {
                return false;
            }
            let keys = [kTISPropertyInputSourceID];
            let values = [cf_id];
            let filter = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(cf_id);
            if filter.is_null() {
                return false;
            }
            let list = TISCreateInputSourceList(filter, 0);
            CFRelease(filter);
            if list.is_null() {
                return false;
            }
            let ok = CFArrayGetCount(list) > 0
                && TISSelectInputSource(CFArrayGetValueAtIndex(list, 0)) == 0;
            CFRelease(list);
            ok
        }
    }
}

/// Select the input source with the given ID. Returns false if it failed
/// (or on non-macOS platforms, where this is a no-op).
pub fn select(id: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        imp::select(id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        false
    }
}
