//! Windows DPAPI wrapper: secrets are bound to the current Windows user account.

#[derive(Debug, thiserror::Error)]
pub enum DpapiError {
    #[error("DPAPI operation failed")]
    Failed,
    #[error("DPAPI is only available on Windows")]
    Unsupported,
}

#[cfg(windows)]
mod imp {
    use super::DpapiError;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    const ENTROPY: &[u8] = b"remotex-secret-store-v1";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// # Safety
    /// `out` must have been filled by a successful DPAPI call.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        v
    }

    pub fn protect(data: &[u8]) -> Result<Vec<u8>, DpapiError> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: blobs point at live slices for the duration of the call; `out` is freed in `take`.
        unsafe {
            CryptProtectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
            .map_err(|_| DpapiError::Failed)?;
            Ok(take(out))
        }
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, DpapiError> {
        let input = blob(data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `protect`.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
            .map_err(|_| DpapiError::Failed)?;
            Ok(take(out))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::DpapiError;
    pub fn protect(_: &[u8]) -> Result<Vec<u8>, DpapiError> {
        Err(DpapiError::Unsupported)
    }
    pub fn unprotect(_: &[u8]) -> Result<Vec<u8>, DpapiError> {
        Err(DpapiError::Unsupported)
    }
}

pub use imp::{protect, unprotect};

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_no_plaintext() {
        let secret = b"correct horse battery staple";
        let enc = protect(secret).unwrap();
        assert!(!enc.windows(secret.len()).any(|w| w == secret));
        assert_eq!(unprotect(&enc).unwrap(), secret);
        assert!(unprotect(b"not a dpapi blob").is_err());
    }
}
