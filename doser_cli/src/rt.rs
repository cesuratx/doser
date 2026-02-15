//! Real-time scheduling helpers (Linux SCHED_FIFO / affinity / mlockall; macOS mlockall).

use crate::cli::RtLock;

#[cfg(target_os = "linux")]
/// Capacity of cpu_set_t in CPU indices (bits).
const MAX_CPUSET_BITS: usize = std::mem::size_of::<libc::cpu_set_t>() * 8;

#[cfg(target_os = "linux")]
pub fn setup_rt_once(rt: bool, prio: Option<i32>, lock: RtLock, rt_cpu: Option<usize>) {
    use libc::{
        CPU_ISSET, CPU_SET, CPU_ZERO, SCHED_FIFO, sched_get_priority_max, sched_get_priority_min,
        sched_param, sched_setscheduler,
    };
    use std::sync::OnceLock;
    static RT_ONCE: OnceLock<()> = OnceLock::new();
    static ONLINE_CPUS: OnceLock<libc::c_long> = OnceLock::new();
    static CPUSET: OnceLock<libc::cpu_set_t> = OnceLock::new();

    if !rt {
        return;
    }

    // Apply process memory locking according to the selected mode.
    #[inline]
    fn try_apply_mem_lock(lock: RtLock) -> eyre::Result<()> {
        #[inline]
        fn is_retryable_memlock_error(err: &std::io::Error) -> bool {
            matches!(err.raw_os_error(), Some(code) if code == libc::EPERM || code == libc::ENOMEM)
        }
        use libc::{MCL_CURRENT, MCL_FUTURE, mlockall};

        #[inline]
        fn memlock_limit_hint() -> Option<String> {
            unsafe {
                let mut rlim = std::mem::MaybeUninit::<libc::rlimit>::uninit();
                let rc = libc::getrlimit(libc::RLIMIT_MEMLOCK, rlim.as_mut_ptr());
                if rc == 0 {
                    let r = rlim.assume_init();
                    let cur = r.rlim_cur;
                    if cur == libc::RLIM_INFINITY {
                        Some("memlock limit: unlimited".to_string())
                    } else {
                        Some(format!("memlock limit: {} KiB", cur / 1024))
                    }
                } else {
                    None
                }
            }
        }

        #[inline]
        fn lock_current() -> std::io::Result<()> {
            let rc = unsafe { mlockall(MCL_CURRENT) };
            if rc != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        #[inline]
        fn lock_all() -> std::io::Result<()> {
            let rc = unsafe { mlockall(MCL_CURRENT | MCL_FUTURE) };
            if rc != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        let attempted_all = matches!(lock, RtLock::All);
        let result: std::io::Result<()> = match lock {
            RtLock::None => Ok(()),
            RtLock::Current => lock_current(),
            RtLock::All => lock_all(),
        };
        if result.is_ok() {
            return Ok(());
        }
        let err = result
            .err()
            .unwrap_or_else(|| std::io::Error::other("mlockall failed"));

        // Fallback: if All failed due to permission or memory, try Current
        let mut fallback_err: Option<std::io::Error> = None;
        if attempted_all && is_retryable_memlock_error(&err) {
            match lock_current() {
                Ok(()) => return Ok(()),
                Err(e2) => fallback_err = Some(e2),
            }
        }

        let mut msg = format!(
            "mlockall({}) failed: {}",
            if attempted_all {
                "current|future"
            } else {
                "current"
            },
            err
        );
        if is_retryable_memlock_error(&err) {
            if let Some(h) = memlock_limit_hint() {
                msg.push_str(&format!("; {h}"));
            }
            msg.push_str("; hint: needs CAP_IPC_LOCK (or root) and sufficient 'ulimit -l'");
            if let Some(e2) = fallback_err {
                msg.push_str(&format!("; fallback mlockall(current) also failed: {e2}"));
            }
        }
        Err(eyre::eyre!(msg))
    }

    // Apply SCHED_FIFO priority, clamped to the system range.
    #[inline]
    fn try_apply_fifo_priority(prio: Option<i32>) -> eyre::Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::fs;
            if let Ok(status) = fs::read_to_string("/proc/self/status") {
                let has_cap = status.lines().any(|line| {
                    if line.starts_with("CapEff:") || line.starts_with("CapPrm:") {
                        if let Some(hex) = line.split_whitespace().nth(1)
                            && let Ok(caps) = u64::from_str_radix(hex, 16)
                        {
                            return caps & 0x800000 != 0;
                        }
                    }
                    false
                });

                if !has_cap {
                    let is_root = unsafe { libc::geteuid() == 0 };
                    if !is_root {
                        return Err(eyre::eyre!(
                            "Insufficient privileges for SCHED_FIFO: needs CAP_SYS_NICE or root. \
                            Current effective UID: {}. \
                            Hint: Run with 'sudo' or grant CAP_SYS_NICE: 'sudo setcap cap_sys_nice=ep /path/to/doser'",
                            unsafe { libc::geteuid() }
                        ));
                    }
                }
            }
        }

        let (min, max) = unsafe {
            let min = sched_get_priority_min(SCHED_FIFO);
            let max = sched_get_priority_max(SCHED_FIFO);
            if min < 0 || max < 0 {
                (1, 99)
            } else {
                (min, max)
            }
        };
        let wanted = prio.unwrap_or(max);
        let prio_val = wanted.clamp(min, max);
        let param = sched_param {
            sched_priority: prio_val,
        };
        let rc = unsafe { sched_setscheduler(0, SCHED_FIFO, &param) };
        if rc != 0 {
            Err(eyre::eyre!(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    // Pin process to a single CPU if permitted by the current affinity mask.
    #[inline]
    fn try_apply_affinity(
        rt_cpu: Option<usize>,
        online_cpus: &OnceLock<libc::c_long>,
        mask: &OnceLock<libc::cpu_set_t>,
    ) -> eyre::Result<()> {
        let _ = online_cpus.get_or_init(|| unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) });
        let _ = mask.get_or_init(|| {
            let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
            unsafe { CPU_ZERO(&mut set) };
            let rc = unsafe {
                libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set)
            };
            if rc != 0 {
                unsafe { CPU_ZERO(&mut set) };
                let n = online_cpus
                    .get()
                    .copied()
                    .unwrap_or_else(|| unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) });
                let n = if n < 0 { 0 } else { n as usize };
                let n = n.min(MAX_CPUSET_BITS);
                for i in 0..n {
                    unsafe { CPU_SET(i, &mut set) };
                }
            }
            set
        });
        let nprocs_onln = *online_cpus.get().unwrap_or(&0);
        if nprocs_onln < 1 {
            eyre::bail!("_SC_NPROCESSORS_ONLN < 1");
        }
        let target = rt_cpu.unwrap_or(0);
        if target as libc::c_long >= nprocs_onln {
            eyre::bail!("requested CPU {target} >= online {nprocs_onln}");
        }
        if target >= MAX_CPUSET_BITS {
            eyre::bail!("requested CPU {target} exceeds cpu_set_t capacity {MAX_CPUSET_BITS}");
        }
        let Some(allowed) = mask.get() else {
            eyre::bail!("cpuset init failed");
        };
        let allowed_target = unsafe { (CPU_ISSET(target, allowed) as libc::c_int) != 0 };
        if !allowed_target {
            eyre::bail!("CPU {target} not permitted by current affinity mask");
        }
        let mut desired: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe {
            CPU_ZERO(&mut desired);
            CPU_SET(target, &mut desired);
        }
        let rc =
            unsafe { libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &desired) };
        if rc != 0 {
            Err(eyre::eyre!(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    RT_ONCE.get_or_init(|| {
        // Memory lock
        match try_apply_mem_lock(lock) {
            Ok(()) => match lock {
                RtLock::None => eprintln!("RT: memory locking disabled (none)"),
                RtLock::Current => eprintln!("RT: memory lock = current"),
                RtLock::All => eprintln!("RT: memory lock = all (current|future)"),
            },
            Err(err) => eprintln!("Warning: mlockall failed: {err}"),
        }
        // FIFO priority
        if let Err(err) = try_apply_fifo_priority(prio) {
            let prio_dbg = prio
                .map(|p| p.to_string())
                .unwrap_or_else(|| "(max)".into());
            eprintln!("Warning: sched_setscheduler(SCHED_FIFO, prio={prio_dbg}) failed: {err}");
        }
        // Affinity
        if let Err(err) = try_apply_affinity(rt_cpu, &ONLINE_CPUS, &CPUSET) {
            eprintln!("Warning: affinity not applied: {err}");
        }
    });
}

#[cfg(target_os = "macos")]
pub fn setup_rt_once(rt: bool, lock: RtLock) {
    use libc::{MCL_CURRENT, MCL_FUTURE, mlockall};
    use std::sync::OnceLock;
    static RT_ONCE: OnceLock<()> = OnceLock::new();
    if !rt {
        return;
    }
    RT_ONCE.get_or_init(|| {
        match lock {
            RtLock::None => {
                eprintln!("RT: memory locking disabled (none)");
            }
            RtLock::Current => {
                let rc = unsafe { mlockall(MCL_CURRENT) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    eprintln!("Warning: mlockall(MCL_CURRENT) failed: {err}");
                } else {
                    eprintln!("RT: memory lock = current");
                }
            }
            RtLock::All => {
                let rc = unsafe { mlockall(MCL_CURRENT | MCL_FUTURE) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    eprintln!("Warning: mlockall(MCL_CURRENT|MCL_FUTURE) failed: {err}");
                } else {
                    eprintln!("RT: memory lock = all (current|future)");
                }
            }
        }
        eprintln!("Warning: macOS does not support SCHED_FIFO or affinity; only mlockall applied.");
    });
}
