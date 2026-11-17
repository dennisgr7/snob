//! Random bytes from the operating system, for tokens, keys and ids.

#[cfg(unix)]
pub fn fill_random(bytes: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read as _;
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(windows)]
pub fn fill_random(bytes: &mut [u8]) -> std::io::Result<()> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    let length = u32::try_from(bytes.len()).map_err(std::io::Error::other)?;
    // SAFETY: the buffer is valid for writes of `length` bytes, and a null
    // algorithm handle is what the system-preferred generator flag asks for.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "BCryptGenRandom failed with {status:#x}"
        )))
    }
}
