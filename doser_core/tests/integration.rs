// Integration test for CLI with missing arguments
#[test]
fn test_cli_missing_arguments() {
    let workspace_root = env!("CARGO_MANIFEST_DIR");
    let cli_manifest = format!("{}/../doser_cli/Cargo.toml", workspace_root);
    let output = Command::new("cargo")
        .args([
            "run",
            "--manifest-path",
            &cli_manifest,
            "--bin",
            "doser_cli",
        ])
        .env("RUST_LOG", "info")
        .output()
        .expect("Failed to run CLI");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("CLI STDOUT (missing args):\n{}", stdout);
    println!("CLI STDERR (missing args):\n{}", stderr);
    assert!(
        stdout.contains("Please specify --grams") || stderr.contains("Please specify --grams"),
        "Expected help message for missing arguments"
    );
}

// Integration test for dosing log file creation and content
#[test]
fn test_dosing_log_file() {
    use std::fs;
    use std::path::Path;
    let workspace_root = env!("CARGO_MANIFEST_DIR");
    let cli_manifest = format!("{}/../doser_cli/Cargo.toml", workspace_root);
    let log_path = format!("{}/dosing_log_test.txt", workspace_root);
    // Remove log file if it exists
    let _ = fs::remove_file(&log_path);
    let output = Command::new("cargo")
        .args([
            "run",
            "--manifest-path",
            &cli_manifest,
            "--bin",
            "doser_cli",
            "--",
            "--grams",
            "5",
            "--log",
            &log_path,
        ])
        .env("RUST_LOG", "info")
        .output()
        .expect("Failed to run CLI");
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("CLI STDOUT (log test):\n{}", stdout);
    // Check log file exists and has expected content
    assert!(Path::new(&log_path).exists(), "Log file was not created");
    let log_content = fs::read_to_string(&log_path).expect("Failed to read log file");
    assert!(
        log_content.contains("target: 5.00"),
        "Log file missing target grams"
    );
    assert!(
        log_content.contains("attempts:"),
        "Log file missing attempts"
    );
    // Clean up
    let _ = fs::remove_file(&log_path);
}
// Integration test for CLI dosing and hardware simulation
use std::process::Command;

#[test]
fn test_cli_simulated_dosing() {
    // Run from workspace root to ensure doser_cli binary is found
    // Use workspace root and absolute path for doser_cli manifest
    let workspace_root = env!("CARGO_MANIFEST_DIR");
    let cli_manifest = format!("{}/../doser_cli/Cargo.toml", workspace_root);
    let output = Command::new("cargo")
        .args([
            "run",
            "--manifest-path",
            &cli_manifest,
            "--bin",
            "doser_cli",
            "--",
            "--grams",
            "5",
        ])
        .env("RUST_LOG", "info")
        .output()
        .expect("Failed to run CLI");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("CLI STDOUT:\n{}", stdout);
    println!("CLI STDERR:\n{}", stderr);
    assert!(stdout.contains("Target grams"), "CLI output: {}", stdout);
    assert!(stdout.contains("Dosing complete"), "CLI output: {}", stdout);
}

// Integration test for hardware error handling (simulated)
#[test]
fn test_simulated_hardware_error() {
    // Simulate a scale that always returns negative weight
    use doser_core::*;
    struct BadScale;
    impl doser_hardware::Scale for BadScale {
        fn read_weight(&mut self) -> f32 {
            -1.0
        }
        fn tare(&mut self) {}
        fn calibrate(&mut self, _known_weight: f32) {}
    }
    struct DummyMotor;
    impl doser_hardware::Motor for DummyMotor {
        fn start(&mut self) {}
        fn stop(&mut self) {}
    }
    let mut doser = Doser::new(Box::new(BadScale), Box::new(DummyMotor), 5.0, 2);
    let result = doser.step();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Negative weight"));
}
