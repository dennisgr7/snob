//! Signing a report, so the receiver can tell it came from here:
//! HMAC-SHA256 over the exact bytes of the body, as `sha256=<hex>` (the shape
//! GitHub uses).
//!
//! It is authenticity, not confidentiality: the body is readable on the path,
//! which is why `http://` outside a private network is refused, not signed.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::secret::Secret;

/// The header the signature travels in.
pub const HEADER: &str = "X-Snob-Signature";

/// Signs `body` and returns the header value.
///
/// The bytes as they will be sent, never a second rendering of a structure: a
/// retry that signed a differently spaced string would be rejected.
pub fn sign(body: &str, key: &Secret) -> String {
    sign_bytes(body.as_bytes(), key.expose().as_bytes())
}

/// Whether two secrets or signatures are equal, in a time that does not
/// depend on where they first differ.
///
/// **The one home for this comparison** (the relay's tokens, the MCP server's
/// tickets and key, [`Secret`]'s `==`). The length is not hidden, and need
/// not be: callers compare values whose length their format fixes, or HMACs.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let differing = a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y));
    // `black_box` keeps the optimizer from turning the fold into an early exit.
    std::hint::black_box(differing) == 0
}

/// [`sign`] over raw bytes, which is what RFC 4231's vectors are given in.
fn sign_bytes(body: &[u8], key: &[u8]) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(body);

    let digest = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(7 + digest.len() * 2);
    hex.push_str("sha256=");
    for byte in digest {
        use std::fmt::Write;
        // Writing into a `String` cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anchored to the standard rather than to this implementation.
    ///
    /// RFC 4231's first test case for HMAC-SHA256. A test that only compared
    /// this code against itself would keep passing through a change that made
    /// the signature something no receiver could verify.
    #[test]
    fn it_matches_the_published_test_vector() {
        assert_eq!(
            sign_bytes(b"Hi There", &[0x0b; 20]),
            "sha256=b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    /// A key longer than the block size is hashed first, which is HMAC's own
    /// rule and the case an implementation is most likely to get wrong. RFC
    /// 4231's sixth vector.
    #[test]
    fn it_handles_a_key_longer_than_the_block() {
        assert_eq!(
            sign_bytes(
                b"Test Using Larger Than Block-Size Key - Hash Key First",
                &[0xaa; 131]
            ),
            "sha256=60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    /// The public entry point and the one the vectors drive have to be the
    /// same function underneath, or the vectors prove nothing about what
    /// actually gets sent.
    #[test]
    fn the_public_signature_is_the_one_the_vectors_check() {
        let key = "a shared secret";
        assert_eq!(
            sign("a body", &Secret::from(key.to_string())),
            sign_bytes(b"a body", key.as_bytes())
        );
    }

    /// One byte different anywhere in the body is a different signature. This
    /// is the whole property, and it is what makes storing the exact bytes
    /// rather than the structure necessary.
    #[test]
    fn a_body_that_differs_by_one_byte_signs_differently() {
        let key = Secret::from("shared".to_string());
        assert_ne!(sign(r#"{"a":1}"#, &key), sign(r#"{"a":2}"#, &key));
    }

    #[test]
    fn a_different_key_signs_differently() {
        let body = r#"{"a":1}"#;
        assert_ne!(
            sign(body, &Secret::from("one".to_string())),
            sign(body, &Secret::from("two".to_string()))
        );
    }

    /// The shape a receiver matches on, so it is asserted rather than assumed.
    #[test]
    fn the_value_is_a_prefixed_hex_digest() {
        let value = sign("anything", &Secret::from("k".to_string()));
        let hex = value
            .strip_prefix("sha256=")
            .expect("the prefix is part of it");
        assert_eq!(hex.len(), 64, "SHA-256 is 32 bytes");
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }

    #[test]
    fn secrets_are_compared_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
        assert!(!same(b"", b"a"));
    }
}
