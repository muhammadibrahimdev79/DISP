#![no_main]

use disp::lexer::Lexer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|source: &str| {
    let _ = Lexer::new(source).tokenize();
});
