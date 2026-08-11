#![no_main]

use disp::check_source;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|source: &str| {
    let _ = check_source(source);
});
