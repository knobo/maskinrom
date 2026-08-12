use serde::Serialize;
use std::time::Duration;
use sysinfo::{Networks, ProcessesToUpdate, System};
use tauri::Emitter;

#[derive(Serialize, Clone)]
struct ProcStat {
    name: String,
    cpu: f32,
    mem: u64,
}

#[derive(Serialize, Clone)]
struct Stats {
    cpu_total: f32,
    cores: Vec<f32>,
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
}

fn collect(sys: &mut System, networks: &mut Networks) -> Stats {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    networks.refresh(true);

    let (net_rx, net_tx) = networks
        .iter()
        .fold((0u64, 0u64), |(rx, tx), (_, data)| {
            (rx + data.received(), tx + data.transmitted())
        });

    let mut procs: Vec<ProcStat> = sys
        .processes()
        .values()
        .map(|p| ProcStat {
            name: p.name().to_string_lossy().into_owned(),
            cpu: p.cpu_usage(),
            mem: p.memory(),
        })
        .collect();
    procs.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    procs.truncate(6);

    let load = System::load_average();

    Stats {
        cpu_total: sys.global_cpu_usage(),
        cores: sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
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
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut sys = System::new_all();
                let mut networks = Networks::new_with_refreshed_list();
                // First CPU reading is meaningless until a second refresh has happened.
                sys.refresh_cpu_usage();
                std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                loop {
                    let stats = collect(&mut sys, &mut networks);
                    let _ = handle.emit("stats", &stats);
                    std::thread::sleep(Duration::from_millis(1000));
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
