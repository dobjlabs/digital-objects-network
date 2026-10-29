//! The thread pool proofs run in, shaped by the `cpuUsage` setting.
//!
//! Each level makes the same promise about the rest of the machine on every
//! platform, through whatever the platform offers: a thread QoS class on
//! macOS, a thread priority plus EcoQoS on Windows, a thread priority on
//! Linux, and a smaller pool wherever those alone do not lower power enough.

use std::sync::{Arc, Mutex};

use rayon::{ThreadPool, ThreadPoolBuilder};
use wire_types::CpuUsage;

/// Logical CPUs per pool thread at the levels below Full.
struct CpusPerThread {
    medium: usize,
    light: usize,
}

const CPUS_PER_THREAD: CpusPerThread = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    // The QoS class alone moves work to efficiency cores and lower clocks.
    CpusPerThread {
        medium: 1,
        light: 1,
    }
} else if cfg!(windows) {
    // EcoQoS does that for Medium; Light keeps Medium's priority and draws
    // less power through fewer threads.
    CpusPerThread {
        medium: 1,
        light: 2,
    }
} else {
    CpusPerThread {
        medium: 2,
        light: 4,
    }
};

/// A proof started while another waits on a pool thread can run nested on
/// that thread's stack, so leave room for several.
const STACK_SIZE: usize = 16 * 1024 * 1024;

/// One pool shared by concurrent proofs, so the level bounds them together.
/// Proofs already running when the level changes finish in the pool they
/// started in.
#[derive(Default)]
pub(crate) struct ProvingPool {
    current: Mutex<Option<(CpuUsage, Arc<ThreadPool>)>>,
}

impl ProvingPool {
    pub(crate) fn get(&self, level: CpuUsage) -> Arc<ThreadPool> {
        let mut current = self.current.lock().expect("proving pool lock poisoned");
        if let Some((current_level, pool)) = current.as_ref()
            && *current_level == level
        {
            return pool.clone();
        }
        let pool = Arc::new(build_proving_pool(level));
        *current = Some((level, pool.clone()));
        pool
    }
}

/// A fresh pool for `level`. Everything run inside it with `install`,
/// including the single-threaded parts of a proof, stays within the level.
pub fn build_proving_pool(level: CpuUsage) -> ThreadPool {
    let logical_cpus = std::thread::available_parallelism().map_or(1, |count| count.get());
    ThreadPoolBuilder::new()
        .num_threads(thread_count(level, logical_cpus))
        .thread_name(|index| format!("prover-{index}"))
        .stack_size(STACK_SIZE)
        .start_handler(move |_| configure_current_thread(level))
        .build()
        .expect("failed to build the proving thread pool")
}

fn thread_count(level: CpuUsage, logical_cpus: usize) -> usize {
    let cpus_per_thread = match level {
        CpuUsage::Full => 1,
        CpuUsage::Medium => CPUS_PER_THREAD.medium,
        CpuUsage::Light => CPUS_PER_THREAD.light,
    };
    logical_cpus.div_ceil(cpus_per_thread)
}

#[cfg(target_os = "macos")]
fn configure_current_thread(level: CpuUsage) {
    use std::ffi::c_int;

    // From <sys/qos.h>; libc does not export the QoS API.
    const QOS_CLASS_UTILITY: u32 = 0x11;
    const QOS_CLASS_BACKGROUND: u32 = 0x09;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    }

    let class = match level {
        CpuUsage::Full => return,
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
fn configure_current_thread(level: CpuUsage) {
    let nice = match level {
        CpuUsage::Full => return,
        CpuUsage::Medium => 10,
        CpuUsage::Light => 19,
    };
    // On Linux the nice value belongs to the thread, and id 0 names the
    // calling thread.
    // SAFETY: takes plain values and only affects the calling thread.
    let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) };
    if result != 0 {
        log::warn!(
            "could not lower the priority of a proving thread: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(windows)]
fn configure_current_thread(level: CpuUsage) {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadInformation, SetThreadPriority,
        THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_BELOW_NORMAL, ThreadPowerThrottling,
    };

    if level == CpuUsage::Full {
        return;
    }
    let eco_qos = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    };
    // SAFETY: the pseudo-handle names the calling thread, and the buffer and
    // its size describe `eco_qos`.
    unsafe {
        let thread = GetCurrentThread();
        if SetThreadPriority(thread, THREAD_PRIORITY_BELOW_NORMAL) == 0 {
            log::warn!(
                "could not lower the priority of a proving thread: {}",
                std::io::Error::last_os_error()
            );
        }
        let set = SetThreadInformation(
            thread,
            ThreadPowerThrottling,
            (&eco_qos as *const THREAD_POWER_THROTTLING_STATE).cast(),
            size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        );
        if set == 0 {
            log::warn!(
                "could not enable EcoQoS on a proving thread: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn configure_current_thread(_level: CpuUsage) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_is_reused_until_the_level_changes() {
        let pools = ProvingPool::default();
        let medium = pools.get(CpuUsage::Medium);
        assert!(Arc::ptr_eq(&medium, &pools.get(CpuUsage::Medium)));
        let light = pools.get(CpuUsage::Light);
        assert!(!Arc::ptr_eq(&medium, &light));
        assert!(Arc::ptr_eq(&light, &pools.get(CpuUsage::Light)));
    }

    #[test]
    fn full_uses_every_cpu_and_lower_levels_never_use_more() {
        for logical_cpus in [1, 2, 3, 10, 32] {
            let full = thread_count(CpuUsage::Full, logical_cpus);
            let medium = thread_count(CpuUsage::Medium, logical_cpus);
            let light = thread_count(CpuUsage::Light, logical_cpus);
            assert_eq!(full, logical_cpus);
            assert!(full >= medium && medium >= light && light >= 1);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pool_threads_carry_the_level() {
        let nice = || unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        assert_eq!(build_proving_pool(CpuUsage::Full).install(nice), nice());
        assert_eq!(build_proving_pool(CpuUsage::Medium).install(nice), 10);
        assert_eq!(build_proving_pool(CpuUsage::Light).install(nice), 19);
    }
}
