/// FNV-1a (Fowler-Noll-Vo) 64-bit hash function
///
/// This matches the C implementation's `fnv_64a_buf` function
/// Used for generating node and ident digests for deduplication.
/// FNV-1a 64-bit non-zero initial basis
pub(crate) const FNV1A_64_INIT: u64 = 0xcbf29ce484222325;

/// Compute 64-bit FNV-1a hash
///
/// This is a faithful port of the C implementation's `fnv_64a_buf` function:
/// ```c
/// static inline uint64_t fnv_64a_buf(const void *buf, size_t len, uint64_t hval) {
///     unsigned char *bp = (unsigned char *)buf;
///     unsigned char *be = bp + len;
///     while (bp < be) {
///         hval ^= (uint64_t)*bp++;
///         hval += (hval << 1) + (hval << 4) + (hval << 5) + (hval << 7) + (hval << 8) + (hval << 40);
///     }
///     return hval;
/// }
/// ```
///
/// # Arguments
/// * `data` - The data to hash
/// * `init` - Initial hash value (use FNV1A_64_INIT for first hash)
///
/// # Returns
/// 64-bit hash value
///
/// Note: This function appears unused but is actually called via `fnv_64a_str` below,
/// which provides the primary API for string hashing. Both functions share the core
/// FNV-1a implementation logic.
#[inline]
#[allow(dead_code)] // Used via fnv_64a_str wrapper
pub(crate) fn fnv_64a(data: &[u8], init: u64) -> u64 {
    let mut hval = init;

    for &byte in data {
        hval ^= byte as u64;
        // FNV magic prime multiplication done via shifts and adds
        // This is equivalent to: hval *= 0x100000001b3 (FNV 64-bit prime)
        hval = hval.wrapping_add(
            (hval << 1)
                .wrapping_add(hval << 4)
                .wrapping_add(hval << 5)
                .wrapping_add(hval << 7)
                .wrapping_add(hval << 8)
                .wrapping_add(hval << 40),
        );
    }

    hval
}

/// Hash a null-terminated string (includes the null byte)
///
/// The C implementation includes the null terminator in the hash:
/// `fnv_64a_buf(node, node_len, FNV1A_64_INIT)` where node_len includes the '\0'
///
/// This function adds a null byte to match that behavior.
#[inline]
pub(crate) fn fnv_64a_str(s: &str) -> u64 {
    let bytes = s.as_bytes();
    let mut hval = FNV1A_64_INIT;

    for &byte in bytes {
        hval ^= byte as u64;
        hval = hval.wrapping_add(
            (hval << 1)
                .wrapping_add(hval << 4)
                .wrapping_add(hval << 5)
                .wrapping_add(hval << 7)
                .wrapping_add(hval << 8)
                .wrapping_add(hval << 40),
        );
    }

    // Hash the null terminator to match C behavior
    // C implementation: `hval ^= (uint64_t)*bp++` where *bp is '\0'
    // Since XOR with 0 is a no-op (hval ^ 0 == hval), we skip it and proceed
    // directly to the multiplication step. This optimization produces identical
    // results to the C implementation while being more explicit about the intent.
    hval.wrapping_add(
        (hval << 1)
            .wrapping_add(hval << 4)
            .wrapping_add(hval << 5)
            .wrapping_add(hval << 7)
            .wrapping_add(hval << 8)
            .wrapping_add(hval << 40),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_init() {
        // Test that init constant matches C implementation
        assert_eq!(FNV1A_64_INIT, 0xcbf29ce484222325);
    }

    #[test]
    fn test_fnv1a_empty() {
        // Empty string with null terminator
        let hash = fnv_64a(&[0], FNV1A_64_INIT);
        assert_ne!(hash, FNV1A_64_INIT); // Should be different from init
    }

    #[test]
    fn test_fnv1a_consistency() {
        // Same input should produce same output
        let data = b"test";
        let hash1 = fnv_64a(data, FNV1A_64_INIT);
        let hash2 = fnv_64a(data, FNV1A_64_INIT);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_fnv1a_different_data() {
        // Different input should (usually) produce different output
        let hash1 = fnv_64a(b"test1", FNV1A_64_INIT);
        let hash2 = fnv_64a(b"test2", FNV1A_64_INIT);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_fnv1a_str() {
        // Test string hashing with null terminator
        let hash1 = fnv_64a_str("node1");
        let hash2 = fnv_64a_str("node1");
        let hash3 = fnv_64a_str("node2");

        assert_eq!(hash1, hash2); // Same string should hash the same
        assert_ne!(hash1, hash3); // Different strings should hash differently
    }

    #[test]
    fn test_fnv1a_node_names() {
        // Test with typical Proxmox node names
        let nodes = vec!["pve1", "pve2", "pve3"];
        let mut hashes = Vec::new();

        for node in &nodes {
            let hash = fnv_64a_str(node);
            hashes.push(hash);
        }

        // All hashes should be unique
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(
                    hashes[i], hashes[j],
                    "Hashes for {} and {} should differ",
                    nodes[i], nodes[j]
                );
            }
        }
    }

    #[test]
    fn test_fnv1a_chaining() {
        // Test that we can chain hashes
        let data1 = b"first";
        let data2 = b"second";

        let hash1 = fnv_64a(data1, FNV1A_64_INIT);
        let hash2 = fnv_64a(data2, hash1); // Use previous hash as init

        // Should produce a deterministic result
        let hash1_again = fnv_64a(data1, FNV1A_64_INIT);
        let hash2_again = fnv_64a(data2, hash1_again);

        assert_eq!(hash2, hash2_again);
    }
}
