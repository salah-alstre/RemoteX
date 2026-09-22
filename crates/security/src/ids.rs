use rand::{rngs::OsRng, TryRngCore};
use remotex_common::{DeviceId, SessionId};

fn fill(buf: &mut [u8]) {
    OsRng
        .try_fill_bytes(buf)
        .expect("operating system RNG unavailable");
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    fill(&mut b);
    b
}

/// Uniform random id in 100_000_000..=999_999_999 using rejection sampling. Carries no hardware information.
pub fn generate_device_id() -> DeviceId {
    const LOW: u32 = 100_000_000;
    const SPAN: u32 = 900_000_000;
    // Largest multiple of SPAN that fits in u32, to avoid modulo bias.
    const LIMIT: u32 = u32::MAX - (u32::MAX % SPAN);
    loop {
        let n = u32::from_le_bytes(random_bytes::<4>());
        if n < LIMIT {
            return DeviceId::from_number(LOW + n % SPAN);
        }
    }
}

pub fn generate_session_id() -> SessionId {
    SessionId(random_bytes::<16>())
}

/// 256-bit device secret used to authenticate a registered installation to the server.
pub fn generate_device_secret() -> Vec<u8> {
    random_bytes::<32>().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn device_ids_are_valid_and_diverse() {
        let mut seen = HashSet::new();
        for _ in 0..2000 {
            let id = generate_device_id();
            assert_eq!(DeviceId::parse(id.as_str()).as_ref(), Some(&id));
            seen.insert(id);
        }
        assert!(seen.len() > 1990);
    }

    #[test]
    fn session_ids_differ() {
        assert_ne!(generate_session_id(), generate_session_id());
    }
}
