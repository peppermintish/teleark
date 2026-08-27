#![no_main]

use libfuzzer_sys::fuzz_target;
use teleark_crypto::{ManifestLimits, VaultMasterKey, open_manifest};

fuzz_target!(|data: &[u8]| {
    let limits = ManifestLimits {
        max_public_header_bytes: 16 * 1024,
        max_encrypted_metadata_bytes: 1024 * 1024,
        max_parts: 4096,
        max_name_bytes: 4096,
        max_relative_path_bytes: 32 * 1024,
        max_mime_type_bytes: 255,
        max_remote_name_bytes: 255,
        max_locator_extension_bytes: 16 * 1024,
        max_metadata_fields: 1024,
        max_metadata_value_bytes: 64 * 1024,
    };
    let key = VaultMasterKey::from_bytes([0xa5; 32]);
    let _ = open_manifest(data, &key, limits);
});
