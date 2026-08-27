#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use teleark_crypto::{FileKey, PartLimits, decrypt_part};

fuzz_target!(|data: &[u8]| {
    let limits = PartLimits {
        max_frame_plaintext: 64 * 1024,
        max_frames_per_part: 256,
        max_part_plaintext: 1024 * 1024,
        max_encoded_part: 2 * 1024 * 1024,
        max_parts_per_package: 4096,
    };
    let key = FileKey::from_bytes([0x5a; 32]);
    let mut source = Cursor::new(data);
    let mut destination = Vec::new();
    let _ = decrypt_part(&mut source, &mut destination, &key, limits);
});
