#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    // We fuzz TOML parsing of Config and ensure it never panics and rejects invalids gracefully.
    // Accept both parse errors and validation errors, but do not allow panics.
    let parsed = toml::from_str::<doser_config::Config>(data);
    match parsed {
        Ok(cfg) => {
            // Ensure validate() does not panic
            let _ = cfg.validate();
        }
        Err(_e) => {
            // parse error is acceptable
        }
    }
});
