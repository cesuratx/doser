use doser_config::load_toml;

#[test]
fn rejects_zero_sample_rate_hz() {
    let toml = r#"
[pins]
hx711_dt = 5
hx711_sck = 6
motor_step = 23
motor_dir = 24

[filter]
ma_window = 3
median_window = 3
sample_rate_hz = 0

[control]
coarse_speed = 1200
fine_speed = 250
slow_at_g = 1.0
hysteresis_g = 0.05
stable_ms = 250
epsilon_g = 0.0

[timeouts]
sample_ms = 150

[safety]
max_run_ms = 60000
max_overshoot_g = 1.0
no_progress_epsilon_g = 0.02
no_progress_ms = 1200
"#;

    let cfg = load_toml(toml).expect("parse TOML");
    let err = cfg.validate().expect_err("should reject sample_rate_hz=0");
    assert!(
        format!("{err}")
            .to_lowercase()
            .contains("sample_rate_hz must be > 0")
    );
}

#[test]
fn accepts_positive_sample_rate_hz() {
    let toml = r#"
[pins]
hx711_dt = 5
hx711_sck = 6
motor_step = 23
motor_dir = 24

[filter]
ma_window = 3
median_window = 3
sample_rate_hz = 25

[timeouts]
sample_ms = 150

[safety]
no_progress_epsilon_g = 0.02
no_progress_ms = 1200
max_run_ms = 60000
max_overshoot_g = 1.0
"#;

    let cfg = load_toml(toml).expect("parse TOML");
    cfg.validate().expect("valid config should pass");
}
