//! Motor jog: spin the motor at a fixed rate for a bounded time, for hardware
//! bring-up and bench testing. No scale and no control loop — just STEP/DIR at a
//! commanded rate, interruptible with Ctrl-C. Generic over `Motor` so it runs on
//! both the real hardware backend and the simulator.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use doser_traits::Motor;

/// Jog `motor` at `sps` steps/sec in the given direction for `duration`, then
/// stop. Returns early — after stopping the motor — if `shutdown` is set (Ctrl-C).
///
/// The motor is moved in and dropped on return, so the hardware backend disables
/// the driver (active-low EN) and joins its stepping thread as it leaves scope.
pub fn run<M>(
    mut motor: M,
    sps: u32,
    clockwise: bool,
    duration: Duration,
    shutdown: Arc<AtomicBool>,
) -> eyre::Result<()>
where
    M: Motor,
{
    let dir_label = if clockwise { "cw" } else { "ccw" };
    motor
        .set_direction(clockwise)
        .map_err(|e| eyre::eyre!("set direction: {e}"))?;
    motor
        .set_speed(sps)
        .map_err(|e| eyre::eyre!("set speed: {e}"))?;

    println!(
        "motor jog: {sps} sps {dir_label} for {} ms — press Ctrl-C to stop early",
        duration.as_millis()
    );
    motor.start().map_err(|e| eyre::eyre!("start motor: {e}"))?;

    // Poll the shutdown flag in short slices so Ctrl-C stops the motor promptly
    // rather than only at the end of the run.
    let deadline = Instant::now() + duration;
    let mut interrupted = false;
    while Instant::now() < deadline {
        if shutdown.load(Ordering::Relaxed) {
            interrupted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    motor.stop().map_err(|e| eyre::eyre!("stop motor: {e}"))?;
    println!(
        "motor jog: {} — stopped",
        if interrupted { "interrupted" } else { "done" }
    );
    Ok(())
}
