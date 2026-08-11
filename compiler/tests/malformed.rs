use disp::{check_source, lexer::Lexer};

#[test]
fn deterministic_malformed_input_smoke_never_panics() {
    const ALPHABET: &[char] = &[
        'a', 'Z', '_', '東', '0', '9', ' ', '\n', '\t', '"', '\'', '\\', '/', '*', '+', '-', '=',
        '!', '<', '>', '&', '|', '(', ')', '{', '}', '[', ']', ',', '.', ':', ';', '\0',
    ];
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for length in 0..512usize {
        let mut source = String::with_capacity(length);
        for _ in 0..length {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            source.push(ALPHABET[(state as usize) % ALPHABET.len()]);
        }
        let _ = Lexer::new(&source).tokenize();
        let _ = check_source(&source);
    }
}
