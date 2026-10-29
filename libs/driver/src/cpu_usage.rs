//! Applies the `cpuUsage` setting as the class of the threads doing proving
//! work: a QoS class on macOS, a thread priority plus EcoQoS on Windows, and
//! a nice value on Linux. Thread counts stay as they are.

use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::{Result, anyhow};
use wire_types::CpuUsage;

static GLOBAL_POOL_LEVEL: AtomicU8 = AtomicU8::new(0);

/// Re-classes the threads of Rayon's global pool, where proofs do their
/// parallel work. Each thread applies the latest level when it gets to the
/// broadcast, so an earlier change cannot land after a later one.
pub fn apply_cpu_usage(level: CpuUsage) {
    GLOBAL_POOL_LEVEL.store(level as u8, Ordering::Relaxed);
    rayon::spawn_broadcast(|_| {
        set_current_thread_cpu_usage(level_from_u8(GLOBAL_POOL_LEVEL.load(Ordering::Relaxed)));
    });
}

fn level_from_u8(value: u8) -> CpuUsage {
    [CpuUsage::Light, CpuUsage::Medium, CpuUsage::Full]
        .into_iter()
        .find(|level| *level as u8 == value)
        .expect("stored from a CpuUsage")
}

/// Runs `work` on a fresh thread classed for `level`, or on this thread at
/// Full. A fresh thread because the calling thread is reused for other work,
/// and on Linux a lowered priority cannot always be raised again.
pub(crate) fn run_at_cpu_usage<T: Send>(
    level: CpuUsage,
    work: impl FnOnce() -> Result<T> + Send,
) -> Result<T> {
    if level == CpuUsage::Full {
        return work();
    }
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("prover".to_string())
            .spawn_scoped(scope, || {
                set_current_thread_cpu_usage(level);
                work()
            })
            .map_err(|err| anyhow!("could not start a proving thread: {err}"))?;
        worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[cfg(target_os = "macos")]
pub fn set_current_thread_cpu_usage(level: CpuUsage) {
    use std::ffi::c_int;

    // From <sys/qos.h>; libc does not export the QoS API.
    const QOS_CLASS_DEFAULT: u32 = 0x15;
    const QOS_CLASS_UTILITY: u32 = 0x11;
    const QOS_CLASS_BACKGROUND: u32 = 0x09;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    }

    let class = match level {
        CpuUsage::Full => QOS_CLASS_DEFAULT,
        CpuUsage::Medium => QOS_CLASS_UTILITY,
        CpuUsage::Light => QOS_CLASS_BACKGROUND,
    };
    // SAFETY: takes plain integers and only affects the calling thread.
    let result = unsafe { pthread_set_qos_class_self_np(class, 0) };
    if result != 0 {
        log::warn!("could not set the QoS class of a proving thread: error {result}");
    }
}

#[cfg(target_os = "linux")]
pub fn set_current_thread_cpu_usage(level: CpuUsage) {
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicBool;

    // The nice value dobjd started with, read from a thread no level has
    // touched yet. Levels never ask for a higher priority than this.
    static STARTING_NICE: OnceLock<i32> = OnceLock::new();
    static RAISE_REFUSED: AtomicBool = AtomicBool::new(false);

    // On Linux the nice value belongs to the thread, and id 0 names the
    // calling thread.
    // SAFETY: getpriority and setpriority take plain values and only read or
    // change the calling thread.
    let current = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
    let starting = *STARTING_NICE.get_or_init(|| current);
    let level_nice = match level {
        CpuUsage::Full => 0,
        CpuUsage::Medium => 10,
        CpuUsage::Light => 19,
    };
    let target = level_nice.max(starting);
    if target == current {
        return;
    }
    // SAFETY: as above.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, target) } == 0 {
        return;
    }
    let error = std::io::Error::last_os_error();
    if target < current {
        if !RAISE_REFUSED.swap(true, Ordering::Relaxed) {
            log::warn!("raising the priority of proving threads needs a dobjd restart: {error}");
        }
    } else {
        log::warn!("could not lower the priority of a proving thread: {error}");
    }
}

#[cfg(windows)]
pub fn set_current_thread_cpu_usage(level: CpuUsage) {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadInformation, SetThreadPriority,
        THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_LOWEST,
        THREAD_PRIORITY_NORMAL, ThreadPowerThrottling,
    };

    let (priority, eco_qos) = match level {
        CpuUsage::Full => (THREAD_PRIORITY_NORMAL, 0),
        CpuUsage::Medium => (
            THREAD_PRIORITY_BELOW_NORMAL,
            THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        ),
        CpuUsage::Light => (
            THREAD_PRIORITY_LOWEST,
            THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        ),
    };
    // A zero control mask hands power throttling back to the system.
    let throttling = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: eco_qos,
        StateMask: eco_qos,
    };
    // SAFETY: the pseudo-handle names the calling thread, and the buffer and
    // its size describe `throttling`.
    unsafe {
        let thread = GetCurrentThread();
        if SetThreadPriority(thread, priority) == 0 {
            log::warn!(
                "could not set the priority of a proving thread: {}",
                std::io::Error::last_os_error()
            );
        }
        let set = SetThreadInformation(
            thread,
            ThreadPowerThrottling,
            (&throttling as *const THREAD_POWER_THROTTLING_STATE).cast(),
            size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        );
        if set == 0 {
            log::warn!(
                "could not set EcoQoS on a proving thread: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn set_current_thread_cpu_usage(_level: CpuUsage) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_round_trip_through_the_stored_byte() {
        for level in [CpuUsage::Light, CpuUsage::Medium, CpuUsage::Full] {
            assert_eq!(level_from_u8(level as u8), level);
        }
    }

    #[test]
    fn work_runs_and_its_error_comes_back() {
        for level in [CpuUsage::Light, CpuUsage::Medium, CpuUsage::Full] {
            assert_eq!(run_at_cpu_usage(level, || Ok(7)).unwrap(), 7);
            let error = run_at_cpu_usage::<()>(level, || Err(anyhow!("failed"))).unwrap_err();
            assert_eq!(error.to_string(), "failed");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn work_runs_at_the_level_nice() {
        let nice = || Ok(unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) });
        let own = nice().unwrap();
        assert_eq!(run_at_cpu_usage(CpuUsage::Full, nice).unwrap(), own);
        assert_eq!(
            run_at_cpu_usage(CpuUsage::Medium, nice).unwrap(),
            10.max(own)
        );
        assert_eq!(
            run_at_cpu_usage(CpuUsage::Light, nice).unwrap(),
            19.max(own)
        );
    }
}
