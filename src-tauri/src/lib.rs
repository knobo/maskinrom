use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{DiskRefreshKind, Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};
use tauri::{Emitter, Manager};

/// State shared between the collector thread and the commands.
#[derive(Default)]
struct Shared {
    /// Open log file while recording is on. Each line is one Stats as JSON.
    rec: Mutex<Option<LineWriter<File>>>,
    /// Full process list, built only while someone has asked for it lately.
    all_procs: Mutex<Vec<ProcRow>>,
    procs_wanted: Mutex<Option<Instant>>,
    /// Set by commands that change power or Bluetooth, so the next tick re-reads them.
    poke: AtomicBool,
}
type App = Arc<Shared>;

fn log_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
        .join("maskinrom/logg")
}

/// Starts a new log file and returns its path, or stops recording and returns "".
#[tauri::command]
fn set_recording(on: bool, app: tauri::State<'_, App>) -> Result<String, String> {
    let mut r = app.rec.lock().map_err(|e| e.to_string())?;
    *r = None;
    if !on {
        return Ok(String::new());
    }
    let dir = log_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = cmd("date", &["+%Y-%m-%d_%H%M%S"]);
    let path = dir.join(format!("{}.jsonl", stamp.trim()));
    let file = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    *r = Some(LineWriter::new(file));
    Ok(path.display().to_string())
}

/// Latest full process list; asking for it keeps the collector building it for a few seconds.
#[tauri::command]
fn processes(app: tauri::State<'_, App>) -> Vec<ProcRow> {
    if let Ok(mut w) = app.procs_wanted.lock() {
        *w = Some(Instant::now());
    }
    app.all_procs.lock().map(|p| p.clone()).unwrap_or_default()
}

const SIGNALS: [&str; 8] = ["TERM", "KILL", "INT", "HUP", "STOP", "CONT", "USR1", "USR2"];

/// Runs a command; on failure returns its stderr (or the spawn error) as the message.
fn run_cmd(prog: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(prog).args(args).output().map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("{prog} feilet ({})", out.status) } else { err })
    }
}

#[tauri::command]
fn send_signal(pid: u32, signal: String, root: bool) -> Result<(), String> {
    if !SIGNALS.contains(&signal.as_str()) {
        return Err(format!("ukjent signal {signal}"));
    }
    let pid = pid.to_string();
    let args = ["kill", "-s", signal.as_str(), pid.as_str()];
    // pkexec shows the desktop's polkit password dialog.
    if root { run_cmd("pkexec", &args) } else { run_cmd(args[0], &args[1..]) }.map(|_| ())
}

#[tauri::command]
fn set_bluetooth(on: bool, app: tauri::State<'_, App>) -> Result<(), String> {
    let out = run_cmd("bluetoothctl", &["power", if on { "on" } else { "off" }])?;
    app.poke.store(true, Ordering::Relaxed);
    // bluetoothctl exits 0 even when the change fails, so read its message.
    if out.contains("succeeded") { Ok(()) } else { Err(out.trim().to_string()) }
}

const PROFILES: [&str; 3] = ["power-saver", "balanced", "performance"];

#[tauri::command]
fn set_power_profile(profile: String, app: tauri::State<'_, App>) -> Result<(), String> {
    if !PROFILES.contains(&profile.as_str()) {
        return Err(format!("ukjent profil {profile}"));
    }
    run_cmd(
        "busctl",
        &["set-property", "org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles",
          "org.freedesktop.UPower.PowerProfiles", "ActiveProfile", "s", &profile],
    )?;
    app.poke.store(true, Ordering::Relaxed);
    Ok(())
}

fn available_governors() -> Vec<String> {
    std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_available_governors")
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect()
}

/// Writing the governor needs root, so it goes through pkexec tee for all CPUs at once.
#[tauri::command]
fn set_governor(governor: String, app: tauri::State<'_, App>) -> Result<(), String> {
    if !available_governors().contains(&governor) {
        return Err(format!("ukjent governor {governor}"));
    }
    let mut files: Vec<String> = std::fs::read_dir("/sys/devices/system/cpu")
        .map_err(|e| e.to_string())?
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let f = format!("/sys/devices/system/cpu/{name}/cpufreq/scaling_governor");
            (name.strip_prefix("cpu")?.parse::<u32>().is_ok() && Path::new(&f).exists()).then_some(f)
        })
        .collect();
    files.sort();
    let mut child = std::process::Command::new("pkexec")
        .arg("tee")
        .args(&files)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("pkexec: {e}"))?;
    child.stdin.take().map(|mut i| i.write_all(governor.as_bytes()));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    app.poke.store(true, Ordering::Relaxed);
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { "avbrutt".into() } else { err })
    }
}

/// One row in the full process list.
#[derive(Serialize, Clone)]
struct ProcRow {
    pid: u32,
    ppid: u32,
    user: String,
    name: String,
    cmd: String,
    cpu: f32,
    mem: u64,
    swap: u64,
    threads: u64,
    core: i64,
    state: String,
    /// Seconds since the Unix epoch.
    start: u64,
}

fn all_procs(sys: &System, users: &Users) -> Vec<ProcRow> {
    let mut rows: Vec<ProcRow> = sys
        .processes()
        .values()
        // Threads show up as processes on Linux; the list is per process.
        .filter(|p| p.thread_kind().is_none())
        .map(|p| {
            let pid = p.pid().as_u32();
            let (state, threads, core) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| parse_stat(&s))
                .unwrap_or_default();
            ProcRow {
                pid,
                ppid: p.parent().map_or(0, |pp| pp.as_u32()),
                user: p
                    .user_id()
                    .and_then(|u| users.get_user_by_id(u))
                    .map(|u| u.name().to_string())
                    .unwrap_or_default(),
                name: p.name().to_string_lossy().into_owned(),
                cmd: p.cmd().iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
                cpu: p.cpu_usage(),
                mem: p.memory(),
                swap: proc_swap(pid),
                threads,
                core: if state.is_empty() { -1 } else { core },
                state,
                start: p.start_time(),
            }
        })
        .collect();
    rows.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    rows
}

/// The `free` columns plus dirty/writeback, from /proc/meminfo, in bytes.
#[derive(Serialize, Clone, Default)]
struct MemInfo {
    free: u64,
    available: u64,
    buffers: u64,
    /// Cached + SReclaimable, as `free` counts it.
    cache: u64,
    shared: u64,
    dirty: u64,
    writeback: u64,
}

fn read_meminfo() -> MemInfo {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb: HashMap<&str, u64> = text
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k, v.trim().trim_end_matches("kB").trim().parse::<u64>().ok()? * 1024))
        })
        .collect();
    let g = |k: &str| kb.get(k).copied().unwrap_or(0);
    MemInfo {
        free: g("MemFree"),
        available: g("MemAvailable"),
        buffers: g("Buffers"),
        cache: g("Cached") + g("SReclaimable"),
        shared: g("Shmem"),
        dirty: g("Dirty"),
        writeback: g("Writeback"),
    }
}

#[derive(Serialize, Clone)]
struct ProcStat {
    pid: u32,
    name: String,
    cpu: f32,
    /// Resident set size (RSS).
    mem: u64,
    swap: u64,
    threads: u64,
    /// CPU the process last ran on, -1 if unknown.
    core: i64,
    /// R running, S sleeping, D waiting on I/O, Z zombie, ...
    state: String,
}

/// State, thread count and last CPU from /proc/<pid>/stat. The command name
/// may itself contain spaces and parens, so fields are counted from the last ')'.
fn parse_stat(stat: &str) -> Option<(String, u64, i64)> {
    let f: Vec<&str> = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().collect();
    // f[0] is field 3 (state); num_threads is field 20, processor field 39.
    Some((f.first()?.to_string(), f.get(17)?.parse().ok()?, f.get(36)?.parse().ok()?))
}

/// VmSwap from /proc/<pid>/status, in bytes.
fn proc_swap(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmSwap:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .map_or(0, |kb| kb * 1024)
}

#[derive(Serialize, Clone)]
struct DiskSpace {
    mount: String,
    dev: String,
    fs: String,
    total: u64,
    avail: u64,
}

/// Per block device, like `iostat -x`: rates over the last interval.
#[derive(Serialize, Clone)]
struct DevIo {
    name: String,
    /// Device-mapper volume on top of another listed disk; leave out of totals.
    virt: bool,
    read_bps: f64,
    write_bps: f64,
    read_iops: f64,
    write_iops: f64,
    /// Average ms per completed request (iostat's await).
    await_ms: f64,
    /// Share of wall time with I/O in flight (iostat's %util).
    util: f64,
}

/// Raw /proc/diskstats counters: reads, sectors read, ms reading,
/// writes, sectors written, ms writing, ms with I/O in flight.
type DiskCounters = [u64; 7];

/// Whole devices only (partitions are not in /sys/block), minus loop/ram/zram.
fn read_diskstats() -> HashMap<String, DiskCounters> {
    let text = std::fs::read_to_string("/proc/diskstats").unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let name = *f.get(2)?;
            if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
                return None;
            }
            if !Path::new("/sys/block").join(name).exists() {
                return None;
            }
            let n = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            Some((name.to_string(), [n(3), n(5), n(6), n(7), n(9), n(10), n(12)]))
        })
        .collect()
}

/// dm-0 is shown by its mapper name (e.g. "secure").
fn dev_label(name: &str) -> String {
    std::fs::read_to_string(format!("/sys/block/{name}/dm/name"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| name.to_string())
}

fn io_rates(
    prev: &HashMap<String, DiskCounters>,
    now: &HashMap<String, DiskCounters>,
    secs: f64,
) -> Vec<DevIo> {
    let mut out: Vec<DevIo> = now
        .iter()
        .filter_map(|(name, c)| {
            let p = prev.get(name)?;
            let d = |i: usize| c[i].saturating_sub(p[i]) as f64;
            let ios = d(0) + d(3);
            Some(DevIo {
                name: dev_label(name),
                virt: name.starts_with("dm-"),
                read_bps: d(1) * 512.0 / secs,
                write_bps: d(4) * 512.0 / secs,
                read_iops: d(0) / secs,
                write_iops: d(3) / secs,
                await_ms: if ios > 0.0 { (d(2) + d(5)) / ios } else { 0.0 },
                util: (d(6) / (secs * 10.0)).min(100.0),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Counters behind vmstat's columns: cpu jiffies, iowait jiffies, ctxt, intr, pswpin, pswpout.
type VmCounters = [u64; 6];

/// Returns the counters plus the instantaneous `r` and `b` columns.
fn read_vm() -> (VmCounters, u64, u64) {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let vm = std::fs::read_to_string("/proc/vmstat").unwrap_or_default();
    let mut c = [0u64; 6];
    let (mut running, mut blocked) = (0, 0);
    for line in stat.lines().chain(vm.lines()) {
        let mut f = line.split_whitespace();
        let key = f.next().unwrap_or("");
        let mut n = || f.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        match key {
            // user nice system idle iowait irq softirq steal
            "cpu" => {
                let v: Vec<u64> = (0..8).map(|_| n()).collect();
                c[0] = v.iter().sum();
                c[1] = v[4];
            }
            "ctxt" => c[2] = n(),
            "intr" => c[3] = n(),
            "pswpin" => c[4] = n(),
            "pswpout" => c[5] = n(),
            "procs_running" => running = n(),
            "procs_blocked" => blocked = n(),
            _ => {}
        }
    }
    (c, running, blocked)
}

#[derive(Serialize, Clone, Default)]
struct VmStat {
    running: u64,
    blocked: u64,
    iowait: f64,
    cs: f64,
    intr: f64,
    swap_in: f64,
    swap_out: f64,
}

fn vm_rates(p: &VmCounters, c: &VmCounters, secs: f64, running: u64, blocked: u64) -> VmStat {
    let d = |i: usize| c[i].saturating_sub(p[i]) as f64;
    VmStat {
        running,
        blocked,
        iowait: if d(0) > 0.0 { d(1) / d(0) * 100.0 } else { 0.0 },
        cs: d(2) / secs,
        intr: d(3) / secs,
        swap_in: d(4) / secs,
        swap_out: d(5) / secs,
    }
}

#[derive(Serialize, Clone)]
struct Iface {
    name: String,
    vpn: bool,
    up: bool,
    rx: u64,
    tx: u64,
    addr: String,
}

/// Some(false) for a physical NIC, Some(true) for a VPN tunnel, None for
/// loopback, bridges, veth and other container plumbing.
fn iface_kind(name: &str) -> Option<bool> {
    let base = Path::new("/sys/class/net").join(name);
    if base.join("device").exists() {
        return Some(false);
    }
    let ty = std::fs::read_to_string(base.join("type")).unwrap_or_default();
    // 65534 = ARPHRD_NONE, used by tun and WireGuard.
    let tunnel = ["wg", "tun", "tailscale", "ppp", "proton", "nordlynx"]
        .iter()
        .any(|p| name.starts_with(p));
    (ty.trim() == "65534" || tunnel).then_some(true)
}

/// Tunnels report operstate "unknown", so fall back to the IFF_UP flag.
fn iface_up(name: &str) -> bool {
    let base = Path::new("/sys/class/net").join(name);
    match std::fs::read_to_string(base.join("operstate")).unwrap_or_default().trim() {
        "up" => true,
        "unknown" => std::fs::read_to_string(base.join("flags"))
            .ok()
            .and_then(|f| u64::from_str_radix(f.trim().trim_start_matches("0x"), 16).ok())
            .is_some_and(|f| f & 1 == 1),
        _ => false,
    }
}

fn ifaces(networks: &Networks) -> Vec<Iface> {
    let mut out: Vec<Iface> = networks
        .iter()
        .filter_map(|(name, data)| {
            let vpn = iface_kind(name)?;
            Some(Iface {
                name: name.clone(),
                vpn,
                up: iface_up(name),
                rx: data.received(),
                tx: data.transmitted(),
                addr: data
                    .ip_networks()
                    .iter()
                    .find(|n| n.addr.is_ipv4())
                    .map(|n| format!("{}/{}", n.addr, n.prefix))
                    .unwrap_or_default(),
            })
        })
        .collect();
    out.sort_by(|a, b| (a.vpn, &a.name).cmp(&(b.vpn, &b.name)));
    out
}

/// Output of a short command, or "" if it is missing or fails.
fn cmd(prog: &str, args: &[&str]) -> String {
    std::process::Command::new(prog)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[derive(Serialize, Clone, Default)]
struct Power {
    profile: String,
    governor: String,
    governors: Vec<String>,
}

#[derive(Serialize, Clone, Default)]
struct Bluetooth {
    present: bool,
    powered: bool,
    devices: Vec<String>,
}

/// Power profile and Bluetooth change rarely and cost a process spawn, so the
/// loop calls this every few seconds only.
fn power_and_bt() -> (Power, Bluetooth) {
    let profile = cmd(
        "busctl",
        &["get-property", "org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles",
          "org.freedesktop.UPower.PowerProfiles", "ActiveProfile"],
    );
    let power = Power {
        // busctl prints: s "performance"
        profile: profile.split('"').nth(1).unwrap_or("").to_string(),
        governor: std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
            .unwrap_or_default()
            .trim()
            .to_string(),
        governors: available_governors(),
    };
    let present = Path::new("/sys/class/bluetooth/hci0").exists();
    let powered = present
        && cmd("busctl", &["get-property", "org.bluez", "/org/bluez/hci0", "org.bluez.Adapter1", "Powered"])
            .trim()
            == "b true";
    let devices = if powered {
        // Lines look like: Device B8:D5:0B:13:B3:29 SRS-XB30
        cmd("bluetoothctl", &["devices", "Connected"])
            .lines()
            .filter_map(|l| l.strip_prefix("Device ")?.split_once(' ').map(|(_, n)| n.to_string()))
            .collect()
    } else {
        Vec::new()
    };
    (power, Bluetooth { present, powered, devices })
}

/// Everything beyond the original CPU/memory/network set.
#[derive(Serialize, Clone)]
struct Extra {
    disks: Vec<DiskSpace>,
    io: Vec<DevIo>,
    vm: VmStat,
    power: Power,
    bt: Bluetooth,
    meminfo: MemInfo,
}

#[derive(Serialize, Clone)]
struct Stats {
    /// Milliseconds since the Unix epoch.
    ts: u64,
    cpu_total: f32,
    cores: Vec<f32>,
    /// Kernel CPU numbers for `cores`; offline CPUs are missing from /proc/stat, so index != id.
    core_ids: Vec<u32>,
    mem_total: u64,
    mem_used: u64,
    swap_total: u64,
    swap_used: u64,
    uptime: u64,
    load1: f64,
    load5: f64,
    load15: f64,
    net_rx: u64,
    net_tx: u64,
    procs: Vec<ProcStat>,
    host: String,
    os: String,
    kernel: String,
    cpu_name: String,
    ifaces: Vec<Iface>,
    #[serde(flatten)]
    extra: Extra,
}

fn disk_space(disks: &mut Disks) -> Vec<DiskSpace> {
    disks.refresh_specifics(true, DiskRefreshKind::nothing().with_storage());
    let mut seen = std::collections::HashSet::new();
    disks
        .iter()
        .filter_map(|d| {
            let dev = d.name().to_string_lossy().into_owned();
            // Real block devices only; skips tmpfs, overlay, snap loops and bind mounts.
            if !dev.starts_with("/dev/") || dev.starts_with("/dev/loop") || d.total_space() == 0 || !seen.insert(dev.clone()) {
                return None;
            }
            Some(DiskSpace {
                mount: d.mount_point().to_string_lossy().into_owned(),
                dev,
                fs: d.file_system().to_string_lossy().into_owned(),
                total: d.total_space(),
                avail: d.available_space(),
            })
        })
        .collect()
}

fn collect(sys: &mut System, networks: &mut Networks, extra: Extra) -> Stats {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_memory()
            .with_cpu()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet),
    );
    networks.refresh(true);

    // Physical NICs only: VPN, bridge and veth traffic also crosses a NIC and would count twice.
    let ifaces = ifaces(networks);
    let (net_rx, net_tx) = ifaces
        .iter()
        .filter(|i| !i.vpn)
        .fold((0u64, 0u64), |(rx, tx), i| (rx + i.rx, tx + i.tx));

    let mut procs: Vec<ProcStat> = sys
        .processes()
        .values()
        .map(|p| ProcStat {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            cpu: p.cpu_usage(),
            mem: p.memory(),
            swap: 0,
            threads: 0,
            core: -1,
            state: String::new(),
        })
        .collect();
    procs.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    procs.truncate(6);
    // Only the six shown are worth the extra /proc reads.
    for p in &mut procs {
        p.swap = proc_swap(p.pid);
        if let Some((state, threads, core)) = std::fs::read_to_string(format!("/proc/{}/stat", p.pid))
            .ok()
            .and_then(|s| parse_stat(&s))
        {
            (p.state, p.threads, p.core) = (state, threads, core);
        }
    }

    let load = System::load_average();

    Stats {
        ts: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
        cpu_total: sys.global_cpu_usage(),
        cores: sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
        core_ids: sys.cpus().iter().map(|c| c.name().trim_start_matches("cpu").parse().unwrap_or(0)).collect(),
        mem_total: sys.total_memory(),
        mem_used: sys.used_memory(),
        swap_total: sys.total_swap(),
        swap_used: sys.used_swap(),
        uptime: System::uptime(),
        load1: load.one,
        load5: load.five,
        load15: load.fifteen,
        net_rx,
        net_tx,
        procs,
        host: System::host_name().unwrap_or_default(),
        os: format!(
            "{} {}",
            System::name().unwrap_or_default(),
            System::os_version().unwrap_or_default()
        ),
        kernel: System::kernel_version().unwrap_or_default(),
        cpu_name: sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_default(),
        ifaces,
        extra,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            set_recording,
            processes,
            send_signal,
            set_bluetooth,
            set_power_profile,
            set_governor
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let shared = App::default();
            app.manage(shared.clone());
            std::thread::spawn(move || {
                let mut sys = System::new_all();
                let mut networks = Networks::new_with_refreshed_list();
                let mut disks = Disks::new();
                let mut prev_io = read_diskstats();
                let mut prev_t = Instant::now();
                let (mut prev_vm, _, _) = read_vm();
                let (mut power, mut bt) = power_and_bt();
                let mut tick: u64 = 0;
                let mut users = Users::new_with_refreshed_list();
                // First CPU reading is meaningless until a second refresh has happened.
                sys.refresh_cpu_usage();
                std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                loop {
                    let now_io = read_diskstats();
                    let now_t = Instant::now();
                    let secs = now_t.duration_since(prev_t).as_secs_f64();
                    let (now_vm, running, blocked) = read_vm();
                    tick += 1;
                    if tick % 5 == 0 || shared.poke.swap(false, Ordering::Relaxed) {
                        (power, bt) = power_and_bt();
                    }
                    let extra = Extra {
                        disks: disk_space(&mut disks),
                        io: io_rates(&prev_io, &now_io, secs),
                        vm: vm_rates(&prev_vm, &now_vm, secs, running, blocked),
                        power: power.clone(),
                        bt: bt.clone(),
                        meminfo: read_meminfo(),
                    };
                    (prev_io, prev_vm, prev_t) = (now_io, now_vm, now_t);
                    let stats = collect(&mut sys, &mut networks, extra);
                    let _ = handle.emit("stats", &stats);
                    let wanted = shared
                        .procs_wanted
                        .lock()
                        .ok()
                        .and_then(|w| *w)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(5));
                    if wanted {
                        if tick % 30 == 0 {
                            users.refresh();
                        }
                        let rows = all_procs(&sys, &users);
                        if let Ok(mut p) = shared.all_procs.lock() {
                            *p = rows;
                        }
                    }
                    if let Some(w) = shared.rec.lock().ok().as_deref_mut().and_then(Option::as_mut) {
                        if let Ok(line) = serde_json::to_string(&stats) {
                            // ponytail: one line per second, no rotation; ~250 MB/day while on.
                            let _ = writeln!(w, "{line}");
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1000));
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_rates_match_iostat() {
        let prev = HashMap::from([("sdz".to_string(), [0, 0, 0, 0, 0, 0, 0])]);
        // Over 2 s: 100 reads of 8 sectors at 300 ms total, 50 writes of 16 sectors at 200 ms, 500 ms busy.
        let now = HashMap::from([("sdz".to_string(), [100, 800, 300, 50, 800, 200, 500])]);
        let r = &io_rates(&prev, &now, 2.0)[0];
        assert_eq!(r.read_bps, 800.0 * 512.0 / 2.0);
        assert_eq!(r.write_bps, 800.0 * 512.0 / 2.0);
        assert_eq!((r.read_iops, r.write_iops), (50.0, 25.0));
        assert!((r.await_ms - 500.0 / 150.0).abs() < 1e-9);
        assert_eq!(r.util, 25.0);
    }

    #[test]
    fn parse_stat_handles_parens_in_name() {
        let mut fields: Vec<String> = (4..=52).map(|i| i.to_string()).collect();
        fields[16] = "7".into(); // field 20: num_threads
        fields[35] = "13".into(); // field 39: processor
        let line = format!("42 (Web (Content) x) S {}", fields.join(" "));
        assert_eq!(parse_stat(&line), Some(("S".to_string(), 7, 13)));
    }
}
