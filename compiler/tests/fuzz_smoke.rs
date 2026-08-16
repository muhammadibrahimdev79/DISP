use disp::{check_source, lexer::Lexer};

fn cases(seed: u64) -> impl Iterator<Item = String> {
    let alphabet = [
        "a", "Z", "0", "9", "_", " ", "\n", "\r", "\t", "{", "}", "(", ")", "[", "]", "<", ">",
        ":", ",", ".", "?", "&", "*", "+", "-", "/", "\"", "'", "é", "λ", "界", "\0",
    ];
    (0..500).scan(seed, move |state, case| {
        let length = case % 193;
        let mut source = String::new();
        for _ in 0..length {
            *state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            source.push_str(alphabet[(*state as usize) % alphabet.len()]);
        }
        Some(source)
    })
}

#[test]
fn lexer_fuzz_smoke_never_panics() {
    for source in cases(0xD15F_A11E) {
        let _ = Lexer::new(&source).tokenize();
    }
}

#[test]
fn complete_frontend_fuzz_smoke_never_panics() {
    for source in cases(0xC0DE_51A7) {
        let _ = check_source(&source);
    }
}

#[cfg(feature = "fuzzing")]
#[test]
fn security_frame_fuzz_smoke_never_panics() {
    let mut state = 0x51EC_0A11_D15F_A11Eu64;
    for case in 0..2_000usize {
        let length = case % 513;
        let mut input = Vec::with_capacity(length);
        for _ in 0..length {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            input.push((state >> 32) as u8);
        }
        let _ = disp::crypto::AeadEnvelope::decode(&input);
        let _ = disp::crypto::decode_ed25519_public_key(&input);
        let _ = disp::crypto::decode_ed25519_signature(&input);
        disp::component_host::fuzz_decode_frame(&input);
        disp::crypto_keystore::fuzz_decode_frames(&input);
    }
}
