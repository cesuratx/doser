#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    // Fuzz the public config loader: it must never panic, and must reject
    // malformed or out-of-range input via `Err` rather than by aborting.
    // Both parse errors and validation errors are acceptable outcomes.
    let parsed: Result<doser_config::Config, toml::de::Error> = doser_config::load_toml(data);
    match parsed {
        Ok(cfg) => {
            // Ensure validate() does not panic on any successfully parsed config.
            let _ = cfg.validate();
        }
        Err(_e) => {
            // parse error is acceptable
        }
    }
});
