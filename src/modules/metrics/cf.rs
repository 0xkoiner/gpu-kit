//! Minimal CoreFoundation and IOKit bindings plus safe owning wrappers.
#![allow(non_camel_case_types, non_upper_case_globals)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::marker::{PhantomData, PhantomPinned};
use std::ops::Deref;
use std::ptr::NonNull;

pub type CFTypeRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type CFArrayRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFDataRef = *const c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;
pub type CFStringEncoding = u32;
pub type CFNumberType = CFIndex;

pub const kCFStringEncodingUTF8: CFStringEncoding = 0x0800_0100;
pub const kCFNumberSInt32Type: CFNumberType = 3;
pub const kCFNumberSInt64Type: CFNumberType = 4;
pub const kCFNumberDoubleType: CFNumberType = 13;

/// An opaque CoreFoundation object. Readers take `&CFType`, so anything they
/// borrow out of it is tied to that reference's lifetime. Raw FFI that mutates
/// an object must not run while such borrows are live.
#[repr(C)]
pub struct CFType {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// An owned CoreFoundation object, released on drop.
pub struct CFOwned(NonNull<CFType>);

// IOKit
pub type io_object_t = u32;
pub type io_iterator_t = u32;
pub type io_registry_entry_t = u32;
pub type io_connect_t = u32;
pub type IOOptionBits = u32;

pub const kIOMainPortDefault: u32 = 0;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFAllocatorDefault: CFAllocatorRef;
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
    pub static kCFBooleanTrue: CFTypeRef;

    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFNumberGetTypeID() -> CFTypeID;
    pub fn CFDictionaryGetTypeID() -> CFTypeID;
    pub fn CFArrayGetTypeID() -> CFTypeID;
    pub fn CFDataGetTypeID() -> CFTypeID;
    pub fn CFBooleanGetTypeID() -> CFTypeID;

    pub fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: CFStringEncoding,
    ) -> CFStringRef;
    pub fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: CFStringEncoding,
    ) -> u8;
    pub fn CFStringGetCStringPtr(string: CFStringRef, encoding: CFStringEncoding) -> *const c_char;
    pub fn CFStringGetLength(string: CFStringRef) -> CFIndex;
    pub fn CFStringGetMaximumSizeForEncoding(
        length: CFIndex,
        encoding: CFStringEncoding,
    ) -> CFIndex;

    pub fn CFDictionaryGetValue(dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    pub fn CFDictionaryGetCount(dict: CFDictionaryRef) -> CFIndex;
    pub fn CFDictionaryCreateMutableCopy(
        alloc: CFAllocatorRef,
        capacity: CFIndex,
        dict: CFDictionaryRef,
    ) -> CFMutableDictionaryRef;
    pub fn CFDictionaryCreateMutable(
        alloc: CFAllocatorRef,
        capacity: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFMutableDictionaryRef;
    pub fn CFDictionarySetValue(dict: CFMutableDictionaryRef, key: CFTypeRef, value: CFTypeRef);

    pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> CFTypeRef;

    pub fn CFNumberGetValue(number: CFNumberRef, kind: CFNumberType, value_ptr: *mut c_void) -> u8;
    pub fn CFNumberCreate(
        alloc: CFAllocatorRef,
        kind: CFNumberType,
        value_ptr: *const c_void,
    ) -> CFNumberRef;

    pub fn CFDataGetLength(data: CFDataRef) -> CFIndex;
    pub fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
}

impl CFType {
    /// Borrows a +0 reference (from a `Get` call) for the caller-chosen `'a`.
    ///
    /// # Safety
    /// `cf` must be null or a CF object that stays alive for `'a`.
    unsafe fn borrowed<'a>(cf: CFTypeRef) -> Option<&'a Self> {
        unsafe { cf.cast::<Self>().as_ref() }
    }

    pub fn as_ptr(&self) -> CFTypeRef {
        (self as *const Self).cast()
    }
}

impl CFOwned {
    /// Takes ownership of a +1 reference (from a `Create`/`Copy` call).
    ///
    /// # Safety
    /// `cf` must be null or a +1 reference to a live CF object that nothing
    /// else will release. Dictionaries and arrays must hold only CF objects,
    /// as those built with the `kCFType` callbacks do.
    pub unsafe fn new(cf: CFTypeRef) -> Option<Self> {
        NonNull::new(cf.cast_mut().cast()).map(Self)
    }

    pub fn string(text: &str) -> Self {
        let text = CString::new(text).expect("CF string without NUL");
        // A `Create` call hands back a +1 reference.
        let cf = unsafe {
            Self::new(CFStringCreateWithCString(
                kCFAllocatorDefault,
                text.as_ptr(),
                kCFStringEncodingUTF8,
            ))
        };
        cf.expect("CFStringCreateWithCString returned NULL")
    }
}

impl Deref for CFOwned {
    type Target = CFType;

    fn deref(&self) -> &CFType {
        // `new`'s contract keeps the object alive while we own it.
        unsafe { self.0.as_ref() }
    }
}

impl Drop for CFOwned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.as_ptr()) };
    }
}

/// Reads a CFString into a Rust `String`.
pub fn string(cf: &CFType) -> Option<String> {
    let cf = cf.as_ptr();
    if unsafe { CFGetTypeID(cf) != CFStringGetTypeID() } {
        return None;
    }
    unsafe {
        let direct = CFStringGetCStringPtr(cf, kCFStringEncodingUTF8);
        if !direct.is_null() {
            return Some(CStr::from_ptr(direct).to_string_lossy().into_owned());
        }
        // Worst-case UTF-8 size plus the NUL; negative means the size overflowed.
        let max = CFStringGetMaximumSizeForEncoding(CFStringGetLength(cf), kCFStringEncodingUTF8);
        let mut buf = vec![0 as c_char; usize::try_from(max).ok()?.checked_add(1)?];
        (CFStringGetCString(
            cf,
            buf.as_mut_ptr(),
            buf.len() as CFIndex,
            kCFStringEncodingUTF8,
        ) != 0)
            .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// Reads a CFNumber as `i64`.
pub fn number_i64(cf: &CFType) -> Option<i64> {
    let cf = cf.as_ptr();
    if unsafe { CFGetTypeID(cf) != CFNumberGetTypeID() } {
        return None;
    }
    let mut value = 0i64;
    (unsafe { CFNumberGetValue(cf, kCFNumberSInt64Type, (&mut value as *mut i64).cast()) } != 0)
        .then_some(value)
}

pub fn is_true(cf: &CFType) -> bool {
    unsafe { cf.as_ptr() == kCFBooleanTrue }
}

/// Looks up `key` in a CFDictionary; the value borrows from `dict`.
pub fn dict_get<'a>(dict: &'a CFType, key: &str) -> Option<&'a CFType> {
    if unsafe { CFGetTypeID(dict.as_ptr()) != CFDictionaryGetTypeID() } {
        return None;
    }
    let key = CFOwned::string(key);
    // The dictionary retains its values (see `CFOwned::new`), so they live as long as `dict`.
    unsafe { CFType::borrowed(CFDictionaryGetValue(dict.as_ptr(), key.as_ptr())) }
}

/// Reads the bytes of a CFData. The slice borrows from `cf`, so it can't
/// outlive the object:
///
/// ```compile_fail,E0597
/// use gpu_ui::modules::metrics::cf::{CFOwned, data_bytes};
/// let bytes = {
///     let data = CFOwned::string("released at the end of this block");
///     data_bytes(&data)
/// };
/// println!("{bytes:?}");
/// ```
pub fn data_bytes(cf: &CFType) -> Option<&[u8]> {
    let cf = cf.as_ptr();
    if unsafe { CFGetTypeID(cf) != CFDataGetTypeID() } {
        return None;
    }
    unsafe {
        let len = CFDataGetLength(cf) as usize;
        let ptr = CFDataGetBytePtr(cf);
        (!ptr.is_null()).then(|| std::slice::from_raw_parts(ptr, len))
    }
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    pub fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    pub fn IOServiceNameMatching(name: *const c_char) -> CFMutableDictionaryRef;
    pub fn IOServiceGetMatchingServices(
        main_port: u32,
        matching: CFDictionaryRef,
        existing: *mut io_iterator_t,
    ) -> i32;
    pub fn IOServiceGetMatchingService(main_port: u32, matching: CFDictionaryRef) -> io_object_t;
    pub fn IOIteratorNext(iterator: io_iterator_t) -> io_object_t;
    pub fn IOObjectRelease(object: io_object_t) -> i32;
    pub fn IORegistryEntryCreateCFProperties(
        entry: io_registry_entry_t,
        properties: *mut CFMutableDictionaryRef,
        allocator: CFAllocatorRef,
        options: IOOptionBits,
    ) -> i32;
    pub fn IORegistryEntryCreateCFProperty(
        entry: io_registry_entry_t,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        options: IOOptionBits,
    ) -> CFTypeRef;
    pub fn IORegistryEntryGetName(entry: io_registry_entry_t, name: *mut c_char) -> i32;
    pub fn IORegistryEntryGetChildIterator(
        entry: io_registry_entry_t,
        plane: *const c_char,
        iterator: *mut io_iterator_t,
    ) -> i32;
    pub fn IOServiceOpen(
        service: io_object_t,
        owning_task: u32,
        kind: u32,
        connect: *mut io_connect_t,
    ) -> i32;
    pub fn IOServiceClose(connect: io_connect_t) -> i32;
    pub fn IOConnectCallStructMethod(
        connection: io_connect_t,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> i32;
}

/// Calls `visit` with every registry entry of class `class`. Entries are
/// released after the call; read only the properties you need with
/// [`property`], because copying a whole property table is costly.
pub fn for_each_service(class: &str, visit: impl FnMut(io_object_t)) {
    let class = CString::new(class).expect("class without NUL");
    let mut iter: io_iterator_t = 0;
    unsafe {
        // `IOServiceGetMatchingServices` consumes the matching dictionary.
        let matching = IOServiceMatching(class.as_ptr());
        if matching.is_null()
            || IOServiceGetMatchingServices(kIOMainPortDefault, matching, &mut iter) != 0
        {
            return;
        }
        visit_iterator(iter, visit);
    }
}

/// Calls `visit` with each child of `entry` in the service plane.
pub fn for_each_child(entry: io_registry_entry_t, visit: impl FnMut(io_object_t)) {
    let mut iter: io_iterator_t = 0;
    unsafe {
        if IORegistryEntryGetChildIterator(entry, c"IOService".as_ptr(), &mut iter) != 0 {
            return;
        }
        visit_iterator(iter, visit);
    }
}

/// Visits and releases every object of `iter`, then the iterator itself.
unsafe fn visit_iterator(iter: io_iterator_t, mut visit: impl FnMut(io_object_t)) {
    unsafe {
        loop {
            let object = IOIteratorNext(iter);
            if object == 0 {
                break;
            }
            visit(object);
            IOObjectRelease(object);
        }
        IOObjectRelease(iter);
    }
}

/// One property of a registry entry.
pub fn property(entry: io_registry_entry_t, key: &str) -> Option<CFOwned> {
    let key = CFOwned::string(key);
    // A `Create` call hands back a +1 reference.
    unsafe {
        CFOwned::new(IORegistryEntryCreateCFProperty(
            entry,
            key.as_ptr(),
            kCFAllocatorDefault,
            0,
        ))
    }
}

/// Iterates the items of a CFArray, borrowed from `array`; yields nothing for
/// any other type.
pub fn array_items(array: &CFType) -> impl Iterator<Item = &CFType> {
    let count = if unsafe { CFGetTypeID(array.as_ptr()) == CFArrayGetTypeID() } {
        unsafe { CFArrayGetCount(array.as_ptr()) }
    } else {
        0
    };
    // The array retains its items (see `CFOwned::new`), so they live as long as `array`.
    (0..count).filter_map(move |ix| unsafe {
        CFType::borrowed(CFArrayGetValueAtIndex(array.as_ptr(), ix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        static kCFTypeArrayCallBacks: c_void;

        fn CFDataCreate(alloc: CFAllocatorRef, bytes: *const u8, length: CFIndex) -> CFDataRef;
        fn CFArrayCreate(
            alloc: CFAllocatorRef,
            values: *const CFTypeRef,
            count: CFIndex,
            callbacks: *const c_void,
        ) -> CFArrayRef;
    }

    fn cf_array(items: &[&str]) -> CFOwned {
        let items: Vec<CFOwned> = items.iter().map(|item| CFOwned::string(item)).collect();
        let ptrs: Vec<CFTypeRef> = items.iter().map(|item| item.as_ptr()).collect();
        let cf = unsafe {
            CFArrayCreate(
                kCFAllocatorDefault,
                ptrs.as_ptr(),
                ptrs.len() as CFIndex,
                (&raw const kCFTypeArrayCallBacks).cast(),
            )
        };
        unsafe { CFOwned::new(cf) }.expect("CFArrayCreate")
    }

    #[test]
    fn array_items_borrows_items_from_the_array() {
        let array = cf_array(&["gpu", "cpu"]);
        let items: Vec<String> = array_items(&array).filter_map(string).collect();
        assert_eq!(items, ["gpu", "cpu"]);
    }

    fn cf_data(bytes: &[u8]) -> CFOwned {
        let cf =
            unsafe { CFDataCreate(kCFAllocatorDefault, bytes.as_ptr(), bytes.len() as CFIndex) };
        unsafe { CFOwned::new(cf) }.expect("CFDataCreate")
    }

    fn cf_dict(key: &str, value: i64) -> CFOwned {
        unsafe {
            let dict = CFDictionaryCreateMutable(
                kCFAllocatorDefault,
                0,
                (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
                (&raw const kCFTypeDictionaryValueCallBacks).cast(),
            );
            let dict = CFOwned::new(dict).expect("CFDictionaryCreateMutable");
            let value = CFNumberCreate(
                kCFAllocatorDefault,
                kCFNumberSInt64Type,
                (&raw const value).cast(),
            );
            let value = CFOwned::new(value).expect("CFNumberCreate");
            CFDictionarySetValue(
                dict.as_ptr().cast_mut(),
                CFOwned::string(key).as_ptr(),
                value.as_ptr(),
            );
            dict
        }
    }

    #[test]
    fn dict_get_borrows_values_from_the_dictionary() {
        let dict = cf_dict("cores", 10);
        assert_eq!(dict_get(&dict, "cores").and_then(number_i64), Some(10));
        assert!(dict_get(&dict, "missing").is_none());
    }

    #[test]
    fn dict_get_returns_none_for_a_non_dictionary() {
        let text = CFOwned::string("not a dictionary");
        assert!(dict_get(&text, "cores").is_none());
    }

    #[test]
    fn data_bytes_reads_an_owned_cfdata() {
        let data = cf_data(b"gpu");
        assert_eq!(data_bytes(&data), Some(&b"gpu"[..]));
    }

    #[test]
    fn string_reads_long_text_through_the_copy_path() {
        // Non-ASCII forces the copy path: CF has no direct UTF-8 pointer for it.
        let text = "é".repeat(200);
        let cf = CFOwned::string(&text);
        assert_eq!(string(&cf), Some(text));
    }
}
