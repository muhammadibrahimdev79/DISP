#![no_main]

use disp::crypto::{
    AeadEnvelope, decode_ed25519_public_key, decode_ed25519_signature,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let _ = AeadEnvelope::decode(input);
    let _ = decode_ed25519_public_key(input);
    let _ = decode_ed25519_signature(input);
    disp::component_host::fuzz_decode_frame(input);
    disp::crypto_keystore::fuzz_decode_frames(input);
});
