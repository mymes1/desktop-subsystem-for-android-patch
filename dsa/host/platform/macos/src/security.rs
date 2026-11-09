//! The small part of CoreFoundation and Security.framework the macOS host calls directly: the
//! keychain's item calls ([`crate::keychain`]) and the check of this program's own
//! entitlements.
//!
//! Every CoreFoundation object made here is owned by a [`Cf`], which releases it once.

use std::ffi::{c_char, c_void};

/// Any CoreFoundation object.
pub type CFTypeRef = *const c_void;

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;

/// CoreFoundation's dictionary callbacks, as laid out in `CFDictionary.h`. Only their address
/// is used.
#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    version: isize,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
    hash: *const c_void,
}

#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    version: isize,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFBooleanTrue: CFTypeRef;
    pub static kCFBooleanFalse: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> CFTypeRef;
    fn CFStringGetLength(string: CFTypeRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(string: CFTypeRef, buffer: *mut c_char, size: isize, encoding: u32)
        -> u8;
    fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
    fn CFDataGetLength(data: CFTypeRef) -> isize;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFTypeRef;
    fn CFGetTypeID(object: CFTypeRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    pub static kSecClass: CFTypeRef;
    pub static kSecClassGenericPassword: CFTypeRef;
    pub static kSecAttrService: CFTypeRef;
    pub static kSecAttrAccount: CFTypeRef;
    pub static kSecAttrLabel: CFTypeRef;
    pub static kSecAttrAccessible: CFTypeRef;
    pub static kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly: CFTypeRef;
    pub static kSecAttrSynchronizable: CFTypeRef;
    pub static kSecValueData: CFTypeRef;
    pub static kSecReturnData: CFTypeRef;
    pub static kSecMatchLimit: CFTypeRef;
    pub static kSecMatchLimitOne: CFTypeRef;
    pub fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> i32;
    pub fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> i32;
    pub fn SecItemUpdate(query: CFTypeRef, update: CFTypeRef) -> i32;
    fn SecCopyErrorMessageString(status: i32, reserved: *mut c_void) -> CFTypeRef;
    fn SecTaskCreateFromSelf(alloc: CFTypeRef) -> CFTypeRef;
    fn SecTaskCopyValueForEntitlement(
        task: CFTypeRef,
        entitlement: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> CFTypeRef;
}

/// A CoreFoundation object this code owns (made by a Create or Copy call), released once when
/// dropped. It may hold nothing.
pub struct Cf(CFTypeRef);

impl Cf {
    /// Takes ownership of `object`, which a Create or Copy call returned (or null).
    ///
    /// # Safety
    /// `object` is null or a CoreFoundation object the caller owns one reference to.
    pub unsafe fn owned(object: CFTypeRef) -> Cf {
        Cf(object)
    }

    pub fn get(&self) -> CFTypeRef {
        self.0
    }

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    /// A `CFString` of `text`.
    pub fn string(text: &str) -> Cf {
        // SAFETY: the bytes are valid UTF-8 of the length passed.
        Cf(unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                text.as_ptr(),
                text.len() as isize,
                UTF8,
                0,
            )
        })
    }

    /// A `CFData` holding a copy of `bytes`.
    pub fn data(bytes: &[u8]) -> Cf {
        // SAFETY: the bytes are valid for the length passed; CFData copies them.
        Cf(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize) })
    }

    /// A `CFDictionary` of `pairs`, which retains every key and value.
    pub fn dictionary(pairs: &[(CFTypeRef, CFTypeRef)]) -> Cf {
        let keys: Vec<CFTypeRef> = pairs.iter().map(|(key, _)| *key).collect();
        let values: Vec<CFTypeRef> = pairs.iter().map(|(_, value)| *value).collect();
        // SAFETY: two arrays of `pairs.len()` CoreFoundation objects, and the CFType
        // callbacks, which retain them.
        Cf(unsafe {
            CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                pairs.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        })
    }

    /// The bytes of a `CFData`; `None` when this is not one.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        if self.0.is_null() {
            return None;
        }
        // SAFETY: a valid CoreFoundation object, checked to be a CFData before it is read.
        unsafe {
            if CFGetTypeID(self.0) != CFDataGetTypeID() {
                return None;
            }
            let length = CFDataGetLength(self.0).max(0) as usize;
            let bytes = CFDataGetBytePtr(self.0);
            if bytes.is_null() || length == 0 {
                return Some(Vec::new());
            }
            Some(std::slice::from_raw_parts(bytes, length).to_vec())
        }
    }

    /// Whether this is `kCFBooleanTrue`.
    pub fn is_true(&self) -> bool {
        if self.0.is_null() {
            return false;
        }
        // SAFETY: a valid CoreFoundation object, checked to be a CFBoolean before it is read.
        unsafe { CFGetTypeID(self.0) == CFBooleanGetTypeID() && CFBooleanGetValue(self.0) != 0 }
    }

    /// The text of a `CFString`; empty when there is none.
    pub fn text(&self) -> String {
        if self.0.is_null() {
            return String::new();
        }
        // SAFETY: a CFString; the buffer holds the most its UTF-8 can take, and a NUL.
        unsafe {
            let length = CFStringGetLength(self.0);
            let size = CFStringGetMaximumSizeForEncoding(length, UTF8) + 1;
            let mut buffer = vec![0 as c_char; size.max(1) as usize];
            if CFStringGetCString(self.0, buffer.as_mut_ptr(), size, UTF8) == 0 {
                return String::new();
            }
            std::ffi::CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this value owns one reference.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// What Security.framework says an `OSStatus` means.
pub fn status_message(status: i32) -> String {
    // SAFETY: the call returns a CFString this code owns, or null.
    unsafe { Cf::owned(SecCopyErrorMessageString(status, std::ptr::null_mut())) }.text()
}

/// Whether this program is signed with the boolean entitlement `name`, set to true.
pub fn has_entitlement(name: &str) -> bool {
    // SAFETY: both calls return objects this code owns, or null; the task is checked first.
    unsafe {
        let task = Cf::owned(SecTaskCreateFromSelf(std::ptr::null()));
        if task.is_null() {
            return false;
        }
        let name = Cf::string(name);
        let value = Cf::owned(SecTaskCopyValueForEntitlement(
            task.get(),
            name.get(),
            std::ptr::null_mut(),
        ));
        value.is_true()
    }
}
