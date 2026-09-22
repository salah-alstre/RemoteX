use crate::ids::random_bytes;

/// Unambiguous alphabet: no 0/O, 1/I/L.
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const TEMP_PASSWORD_LEN: usize = 6;

/// Cryptographically random temporary password, e.g. `K7F92Q`.
pub fn generate_temp_password() -> String {
    let n = ALPHABET.len() as u16;
    let limit = 256 - (256 % n);
    let mut out = String::with_capacity(TEMP_PASSWORD_LEN);
    while out.len() < TEMP_PASSWORD_LEN {
        for b in random_bytes::<16>() {
            if (b as u16) < limit && out.len() < TEMP_PASSWORD_LEN {
                out.push(ALPHABET[(b as u16 % n) as usize] as char);
            }
        }
    }
    out
}

/// Users type passwords in any case with stray spaces.
pub fn normalize_temp_password(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_and_alphabet() {
        for _ in 0..500 {
            let p = generate_temp_password();
            assert_eq!(p.len(), TEMP_PASSWORD_LEN);
            assert!(p.bytes().all(|b| ALPHABET.contains(&b)));
        }
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize_temp_password(" k7f-92q "), "K7F92Q");
    }
}
