#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let parsed = tslib_chat::bbcode::BBCodeParser::new();
        let _ = parsed.to_html(text);
        let _ = parsed.to_plain(text);
        let _ = parsed.to_ansi(text);
        let _ = tslib_chat::bbcode::strip_bbcode(text);
    }
});
