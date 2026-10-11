//! aws-lc gets its entropy from getentropy(3), which NetBSD's libc has only
//! since NetBSD 10, while cross links against NetBSD 9. arc4random_buf(3) is in
//! every supported NetBSD and is seeded from the same kernel source.

use core::ffi::{c_int, c_void};

unsafe extern "C" {
    fn arc4random_buf(buf: *mut c_void, nbytes: usize);
}

/// getentropy(3) refuses requests above this size.
const GETENTROPY_MAX: usize = 256;

#[unsafe(no_mangle)]
extern "C" fn getentropy(buf: *mut c_void, buflen: usize) -> c_int {
    if buflen > GETENTROPY_MAX {
        return -1;
    }
    // SAFETY: the caller passes a buffer that is valid for `buflen` bytes.
    unsafe { arc4random_buf(buf, buflen) };
    0
}
