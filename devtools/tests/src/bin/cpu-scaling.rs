//! Measures how proving responds to the size of the Rayon pool it runs in and, on macOS, to the
//! QoS class of that pool's threads. Each sample is one real FindLog proof, proved and shrunk
//! entirely inside a fresh pool. Every plan entry runs once per repetition, interleaved so thermal
//! drift spreads across entries; the report gives medians.
//!
//! Usage: `cpu-scaling [--reps N] [--csv PATH] [ENTRY ...]`
//!
//! An ENTRY is a `cpuUsage` level (`light`, `medium`, `full`), measured in the pool dobjd proves
//! in at that level, or `QOS:THREADS` for a pool of its own. QOS is `inherit` (threads keep the
//! QoS they are spawned with), or on macOS one of `user-initiated`, `default`, `utility`,
//! `background`. THREADS is a count, `all` for every logical CPU, or on macOS `p` / `e` for the
//! performance / efficiency core count. With no entries it runs `inherit` at 1, 2, 4, ... threads
//! (and on macOS at `p`), then the three levels.
//!
//! Energy comes from the process's own counter on macOS, and on Linux from the RAPL package
//! counters, which cover the whole CPU package and need root.
//!
//! No network: the proofs ground against a dummy state header, which FindLog (no inputs) accepts.

use std::{
    collections::HashMap,
    env, fs, iter,
    time::{Duration, Instant},
};

use driver::build_proving_pool;
use payload::shrink::ShrunkMainPodSetup;
use pod2::middleware::{EMPTY_HASH, Params};
use tests::{Prover, find_log_action};
use txlib::{GroundingWitness, StateHeader};
use wire_types::CpuUsage;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Qos {
    Inherit,
    UserInitiated,
    Default,
    Utility,
    Background,
}

impl Qos {
    fn parse(name: &str) -> Qos {
        let qos = match name {
            "inherit" => Qos::Inherit,
            "user-initiated" => Qos::UserInitiated,
            "default" => Qos::Default,
            "utility" => Qos::Utility,
            "background" => Qos::Background,
            _ => panic!("unknown QoS {name:?}"),
        };
        assert!(
            qos == Qos::Inherit || cfg!(target_os = "macos"),
            "QoS {name:?} is only available on macOS"
        );
        qos
    }

    fn name(self) -> &'static str {
        match self {
            Qos::Inherit => "inherit",
            Qos::UserInitiated => "user-initiated",
            Qos::Default => "default",
            Qos::Utility => "utility",
            Qos::Background => "background",
        }
    }

    fn apply_to_current_thread(self) {
        #[cfg(target_os = "macos")]
        {
            let class = match self {
                Qos::Inherit => return,
                Qos::UserInitiated => macos::QOS_CLASS_USER_INITIATED,
                Qos::Default => macos::QOS_CLASS_DEFAULT,
                Qos::Utility => macos::QOS_CLASS_UTILITY,
                Qos::Background => macos::QOS_CLASS_BACKGROUND,
            };
            macos::set_current_thread_qos(class);
        }
    }
}

struct Topology {
    logical_cpus: usize,
    performance_cpus: Option<usize>,
    efficiency_cpus: Option<usize>,
}

impl Topology {
    fn detect() -> Topology {
        let logical_cpus = std::thread::available_parallelism().unwrap().get();
        #[cfg(target_os = "macos")]
        if macos::sysctl_int(c"hw.nperflevels") == Some(2) {
            let count = |name| macos::sysctl_int(name).map(|count| count as usize);
            return Topology {
                logical_cpus,
                performance_cpus: count(c"hw.perflevel0.logicalcpu"),
                efficiency_cpus: count(c"hw.perflevel1.logicalcpu"),
            };
        }
        Topology {
            logical_cpus,
            performance_cpus: None,
            efficiency_cpus: None,
        }
    }

    fn parse_threads(&self, spec: &str) -> usize {
        match spec {
            "all" => self.logical_cpus,
            "p" => self
                .performance_cpus
                .expect("no performance core count on this machine"),
            "e" => self
                .efficiency_cpus
                .expect("no efficiency core count on this machine"),
            count => {
                let count: usize = count
                    .parse()
                    .unwrap_or_else(|_| panic!("bad thread count {count:?}"));
                assert!(count > 0, "thread count must be positive");
                count
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlanEntry {
    Level(CpuUsage),
    Pool { qos: Qos, threads: usize },
}

impl PlanEntry {
    fn label(self) -> String {
        match self {
            PlanEntry::Level(level) => level.to_string(),
            PlanEntry::Pool { qos, threads } => format!("{}:{threads}", qos.name()),
        }
    }

    fn build_pool(self) -> rayon::ThreadPool {
        match self {
            PlanEntry::Level(level) => build_proving_pool(level).unwrap(),
            PlanEntry::Pool { qos, threads } => rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .start_handler(move |_| qos.apply_to_current_thread())
                .build()
                .unwrap(),
        }
    }
}

const SINGLE_THREAD: PlanEntry = PlanEntry::Pool {
    qos: Qos::Inherit,
    threads: 1,
};

fn default_plan(topology: &Topology) -> Vec<PlanEntry> {
    let mut thread_counts: Vec<usize> = iter::successors(Some(1), |count| Some(count * 2))
        .take_while(|count| *count < topology.logical_cpus)
        .chain(topology.performance_cpus)
        .filter(|count| *count < topology.logical_cpus)
        .collect();
    thread_counts.sort();
    thread_counts.dedup();
    thread_counts
        .into_iter()
        .map(|threads| PlanEntry::Pool {
            qos: Qos::Inherit,
            threads,
        })
        .chain([CpuUsage::Full, CpuUsage::Medium, CpuUsage::Light].map(PlanEntry::Level))
        .collect()
}

struct Options {
    reps: usize,
    csv_path: Option<String>,
    plan: Vec<PlanEntry>,
}

fn parse_options(topology: &Topology) -> Options {
    let mut reps = 3;
    let mut csv_path = None;
    let mut plan = Vec::new();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--reps" => {
                reps = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .expect("--reps takes a count");
            }
            "--csv" => csv_path = Some(args.next().expect("--csv takes a path")),
            entry => match entry.parse() {
                Ok(level) => plan.push(PlanEntry::Level(level)),
                Err(_) => {
                    let (qos, threads) = entry.split_once(':').unwrap_or_else(|| {
                        panic!("plan entries are a level or QOS:THREADS, got {entry:?}")
                    });
                    plan.push(PlanEntry::Pool {
                        qos: Qos::parse(qos),
                        threads: topology.parse_threads(threads),
                    });
                }
            },
        }
    }
    if plan.is_empty() {
        plan = default_plan(topology);
    }
    Options {
        reps,
        csv_path,
        plan,
    }
}

/// Cumulative process counters; samples are differences of two readings.
#[derive(Clone)]
struct Counters {
    cpu_seconds: f64,
    #[cfg(target_os = "macos")]
    rusage: Option<macos::RusageCounters>,
    #[cfg(target_os = "linux")]
    rapl: Option<Vec<linux::RaplReading>>,
}

impl Counters {
    fn read() -> Counters {
        Counters {
            cpu_seconds: process_cpu_seconds(),
            #[cfg(target_os = "macos")]
            rusage: macos::rusage(),
            #[cfg(target_os = "linux")]
            rapl: linux::read_rapl(),
        }
    }
}

#[cfg(unix)]
fn process_cpu_seconds() -> f64 {
    // SAFETY: getrusage only writes the struct it is given.
    let usage = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
        usage
    };
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1e6;
    seconds(usage.ru_utime) + seconds(usage.ru_stime)
}

#[cfg(not(unix))]
fn process_cpu_seconds() -> f64 {
    f64::NAN
}

struct EnergySample {
    joules: f64,
    /// macOS only.
    performance_core_share: Option<f64>,
    /// macOS only.
    billed_qos: Option<&'static str>,
}

struct Sample {
    entry: PlanEntry,
    rep: usize,
    threads: usize,
    wall_seconds: f64,
    cpu_seconds: f64,
    energy: Option<EnergySample>,
}

fn energy_sample(before: &Counters, after: &Counters) -> Option<EnergySample> {
    #[cfg(target_os = "macos")]
    if let (Some(before), Some(after)) = (before.rusage, after.rusage) {
        return Some(macos::energy_sample(&before, &after));
    }
    #[cfg(target_os = "linux")]
    if let (Some(before), Some(after)) = (&before.rapl, &after.rapl) {
        return Some(EnergySample {
            joules: linux::rapl_joules_between(before, after),
            performance_core_share: None,
            billed_qos: None,
        });
    }
    let _ = (before, after);
    None
}

fn measure(prover: &Prover, entry: PlanEntry, rep: usize) -> Sample {
    let pool = entry.build_pool();
    let before = Counters::read();
    let started = Instant::now();
    pool.install(|| prove_find_log(prover));
    let wall = started.elapsed();
    let after = Counters::read();
    Sample {
        entry,
        rep,
        threads: pool.current_num_threads(),
        wall_seconds: wall.as_secs_f64(),
        cpu_seconds: after.cpu_seconds - before.cpu_seconds,
        energy: energy_sample(&before, &after),
    }
}

fn prove_find_log(prover: &Prover) {
    let witness = GroundingWitness::new(
        StateHeader::new(1, 1, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH),
        HashMap::new(),
    );
    prover.prove(&find_log_action(), witness, Vec::new());
    // dobjd builds the shrink circuit afresh for every proof; so does a sample.
    ShrunkMainPodSetup::new(&Params::default()).build().unwrap();
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}

fn median_of(samples: &[&Sample], value: impl Fn(&Sample) -> f64) -> f64 {
    median(samples.iter().map(|sample| value(sample)).collect())
}

fn print_sample(sample: &Sample, num_samples: usize, index: usize) {
    let mut line = format!(
        "[{}/{num_samples}] {:<18} {:>3} threads  wall {:>7.1} s  cpu {:>8.1} s",
        index + 1,
        sample.entry.label(),
        sample.threads,
        sample.wall_seconds,
        sample.cpu_seconds,
    );
    if let Some(energy) = &sample.energy {
        line += &format!("  energy {:>7.1} J", energy.joules);
    }
    if let Some(energy) = &sample.energy
        && let (Some(share), Some(billed)) = (energy.performance_core_share, energy.billed_qos)
    {
        line += &format!("  P-core {:>5.1}%  billed {billed}", share * 100.0);
    }
    println!("{line}");
}

fn print_report(plan: &[PlanEntry], samples: &[Sample]) {
    let single_thread_wall = samples
        .iter()
        .filter(|sample| sample.entry == SINGLE_THREAD)
        .map(|sample| sample.wall_seconds)
        .collect::<Vec<_>>();
    let single_thread_wall = (!single_thread_wall.is_empty()).then(|| median(single_thread_wall));

    println!();
    println!(
        "{:<18} {:>7} {:>8} {:>8} {:>8} {:>10} {:>9} {:>7} {:>8}  billed QoS",
        "plan",
        "threads",
        "wall s",
        "speedup",
        "cpu s",
        "busy cores",
        "energy J",
        "avg W",
        "P-core %"
    );
    for entry in plan {
        let entry_samples: Vec<&Sample> = samples
            .iter()
            .filter(|sample| sample.entry == *entry)
            .collect();
        let wall = median_of(&entry_samples, |sample| sample.wall_seconds);
        let speedup =
            single_thread_wall.map_or("-".to_string(), |single| format!("{:.2}", single / wall));
        let mut line = format!(
            "{:<18} {:>7} {:>8.1} {:>8} {:>8.1} {:>10.2}",
            entry.label(),
            entry_samples[0].threads,
            wall,
            speedup,
            median_of(&entry_samples, |sample| sample.cpu_seconds),
            median_of(&entry_samples, |sample| sample.cpu_seconds
                / sample.wall_seconds),
        );
        if entry_samples.iter().all(|sample| sample.energy.is_some()) {
            let energy = |sample: &Sample| sample.energy.as_ref().unwrap().joules;
            line += &format!(
                " {:>9.1} {:>7.1}",
                median_of(&entry_samples, energy),
                median_of(&entry_samples, |sample| energy(sample)
                    / sample.wall_seconds),
            );
            let first = entry_samples[0].energy.as_ref().unwrap();
            if let (Some(_), Some(billed)) = (first.performance_core_share, first.billed_qos) {
                let share = |sample: &Sample| {
                    let energy = sample.energy.as_ref().unwrap();
                    energy.performance_core_share.unwrap_or(f64::NAN) * 100.0
                };
                line += &format!(" {:>8.1}  {billed}", median_of(&entry_samples, share));
            }
        }
        println!("{line}");
    }
}

fn write_csv(path: &str, samples: &[Sample]) {
    let mut csv = "rep,plan,threads,wall_s,cpu_s,energy_j,p_core_share,billed_qos\n".to_string();
    for sample in samples {
        let energy = sample.energy.as_ref();
        let joules = energy.map_or(String::new(), |energy| energy.joules.to_string());
        let share = energy
            .and_then(|energy| energy.performance_core_share)
            .map_or(String::new(), |share| share.to_string());
        let billed = energy.and_then(|energy| energy.billed_qos).unwrap_or("");
        csv += &format!(
            "{},{},{},{},{},{joules},{share},{billed}\n",
            sample.rep,
            sample.entry.label(),
            sample.threads,
            sample.wall_seconds,
            sample.cpu_seconds,
        );
    }
    fs::write(path, csv).unwrap();
}

fn main() {
    env_logger::init();
    let topology = Topology::detect();
    let options = parse_options(&topology);
    println!(
        "logical CPUs: {}, performance: {}, efficiency: {}",
        topology.logical_cpus,
        topology
            .performance_cpus
            .map_or("unknown".to_string(), |count| count.to_string()),
        topology
            .efficiency_cpus
            .map_or("unknown".to_string(), |count| count.to_string()),
    );
    let labels: Vec<String> = options.plan.iter().map(|entry| entry.label()).collect();
    println!("plan: {} x {} reps", labels.join(" "), options.reps);
    #[cfg(target_os = "linux")]
    if Counters::read().rapl.is_none() {
        println!("no RAPL package energy readable; run as root to report energy");
    }

    println!("building the shrink circuit...");
    let prover = Prover::new();
    println!("warm-up proof...");
    let warm_up = Instant::now();
    prove_find_log(&prover);
    println!("warm-up proof took {:.1?}", warm_up.elapsed());

    let num_samples = options.reps * options.plan.len();
    let mut samples = Vec::with_capacity(num_samples);
    for rep in 0..options.reps {
        for entry in &options.plan {
            let sample = measure(&prover, *entry, rep);
            print_sample(&sample, num_samples, samples.len());
            samples.push(sample);
            // Let the clocks recover between samples.
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    print_report(&options.plan, &samples);
    if let Some(path) = &options.csv_path {
        write_csv(path, &samples);
        println!("wrote {path}");
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CStr, c_int, c_void};

    use super::EnergySample;

    pub const QOS_CLASS_USER_INITIATED: u32 = 0x19;
    pub const QOS_CLASS_DEFAULT: u32 = 0x15;
    pub const QOS_CLASS_UTILITY: u32 = 0x11;
    pub const QOS_CLASS_BACKGROUND: u32 = 0x09;

    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    }

    pub fn set_current_thread_qos(class: u32) {
        // SAFETY: takes plain integers and only affects the calling thread.
        let result = unsafe { pthread_set_qos_class_self_np(class, 0) };
        assert_eq!(
            result, 0,
            "pthread_set_qos_class_self_np({class:#x}) failed"
        );
    }

    pub fn sysctl_int(name: &CStr) -> Option<i32> {
        let mut value: i32 = 0;
        let mut size = size_of::<i32>();
        // SAFETY: the output buffer and its size describe `value`.
        let result = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut i32).cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0).then_some(value)
    }

    /// `struct rusage_info_v6` from `<sys/resource.h>`; libc binds only up to v4.
    #[repr(C)]
    #[derive(Default)]
    struct RusageInfoV6 {
        ri_uuid: [u8; 16],
        ri_user_time: u64,
        ri_system_time: u64,
        ri_pkg_idle_wkups: u64,
        ri_interrupt_wkups: u64,
        ri_pageins: u64,
        ri_wired_size: u64,
        ri_resident_size: u64,
        ri_phys_footprint: u64,
        ri_proc_start_abstime: u64,
        ri_proc_exit_abstime: u64,
        ri_child_user_time: u64,
        ri_child_system_time: u64,
        ri_child_pkg_idle_wkups: u64,
        ri_child_interrupt_wkups: u64,
        ri_child_pageins: u64,
        ri_child_elapsed_abstime: u64,
        ri_diskio_bytesread: u64,
        ri_diskio_byteswritten: u64,
        ri_cpu_time_qos_default: u64,
        ri_cpu_time_qos_maintenance: u64,
        ri_cpu_time_qos_background: u64,
        ri_cpu_time_qos_utility: u64,
        ri_cpu_time_qos_legacy: u64,
        ri_cpu_time_qos_user_initiated: u64,
        ri_cpu_time_qos_user_interactive: u64,
        ri_billed_system_time: u64,
        ri_serviced_system_time: u64,
        ri_logical_writes: u64,
        ri_lifetime_max_phys_footprint: u64,
        ri_instructions: u64,
        ri_cycles: u64,
        ri_billed_energy: u64,
        ri_serviced_energy: u64,
        ri_interval_max_phys_footprint: u64,
        ri_runnable_time: u64,
        ri_flags: u64,
        ri_user_ptime: u64,
        ri_system_ptime: u64,
        ri_pinstructions: u64,
        ri_pcycles: u64,
        ri_energy_nj: u64,
        ri_penergy_nj: u64,
        ri_secure_time_in_system: u64,
        ri_secure_ptime_in_system: u64,
        ri_reserved: [u64; 12],
    }

    const RUSAGE_INFO_V6: c_int = 6;

    const QOS_NAMES: [&str; 7] = [
        "default",
        "maintenance",
        "background",
        "utility",
        "legacy",
        "user-initiated",
        "user-interactive",
    ];

    /// CPU times share one unit (Mach ticks on Apple silicon), so only their ratios are used.
    #[derive(Clone, Copy)]
    pub struct RusageCounters {
        energy_nj: u64,
        cpu_time: u64,
        performance_core_time: u64,
        cpu_time_by_qos: [u64; 7],
    }

    pub fn rusage() -> Option<RusageCounters> {
        let mut info = RusageInfoV6::default();
        // SAFETY: the buffer is a `rusage_info_v6`, the flavor requested.
        let result = unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                RUSAGE_INFO_V6,
                (&mut info as *mut RusageInfoV6).cast::<libc::rusage_info_t>(),
            )
        };
        (result == 0).then(|| RusageCounters {
            energy_nj: info.ri_energy_nj,
            cpu_time: info.ri_user_time + info.ri_system_time,
            performance_core_time: info.ri_user_ptime + info.ri_system_ptime,
            cpu_time_by_qos: [
                info.ri_cpu_time_qos_default,
                info.ri_cpu_time_qos_maintenance,
                info.ri_cpu_time_qos_background,
                info.ri_cpu_time_qos_utility,
                info.ri_cpu_time_qos_legacy,
                info.ri_cpu_time_qos_user_initiated,
                info.ri_cpu_time_qos_user_interactive,
            ],
        })
    }

    pub fn energy_sample(before: &RusageCounters, after: &RusageCounters) -> EnergySample {
        let cpu_time = after.cpu_time - before.cpu_time;
        let performance_core_time = after.performance_core_time - before.performance_core_time;
        let performance_core_share = performance_core_time as f64 / cpu_time as f64;
        if performance_core_share > 1.01 {
            eprintln!(
                "warning: P-core time exceeds total CPU time; the rusage_info_v6 layout here \
                 does not match this macOS, so energy and P-core figures are unreliable"
            );
        }
        let billed = (0..QOS_NAMES.len())
            .max_by_key(|&index| after.cpu_time_by_qos[index] - before.cpu_time_by_qos[index])
            .unwrap();
        EnergySample {
            joules: (after.energy_nj - before.energy_nj) as f64 / 1e9,
            performance_core_share: Some(performance_core_share),
            billed_qos: Some(QOS_NAMES[billed]),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;

    /// One RAPL package domain's cumulative energy and the value it wraps at, in microjoules.
    #[derive(Clone)]
    pub struct RaplReading {
        energy_uj: u64,
        max_energy_range_uj: u64,
    }

    /// The top-level package domains (`intel-rapl:N`, also used for AMD), or `None` when there
    /// are none or they are unreadable, as they are without root.
    pub fn read_rapl() -> Option<Vec<RaplReading>> {
        let mut domains: Vec<_> = fs::read_dir("/sys/class/powercap")
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_prefix("intel-rapl:"))
                    .is_some_and(|index| !index.contains(':'))
            })
            .collect();
        domains.sort();
        let read = |path: &std::path::Path, file| -> Option<u64> {
            fs::read_to_string(path.join(file))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        let readings: Option<Vec<_>> = domains
            .iter()
            .map(|path| {
                Some(RaplReading {
                    energy_uj: read(path, "energy_uj")?,
                    max_energy_range_uj: read(path, "max_energy_range_uj")?,
                })
            })
            .collect();
        readings.filter(|readings| !readings.is_empty())
    }

    pub fn rapl_joules_between(before: &[RaplReading], after: &[RaplReading]) -> f64 {
        let microjoules: u64 = before
            .iter()
            .zip(after)
            .map(|(before, after)| {
                if after.energy_uj >= before.energy_uj {
                    after.energy_uj - before.energy_uj
                } else {
                    after.energy_uj + before.max_energy_range_uj - before.energy_uj
                }
            })
            .sum();
        microjoules as f64 / 1e6
    }
}
