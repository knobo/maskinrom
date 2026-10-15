<script>
  import { onMount } from "svelte";

  const HIST = 90;

  /**
   * @typedef {{ pid: number, name: string, cpu: number, mem: number, swap: number, threads: number, core: number, state: string }} ProcStat
   * @typedef {{ mount: string, dev: string, fs: string, total: number, avail: number }} DiskSpace
   * @typedef {{ name: string, virt: boolean, read_bps: number, write_bps: number, read_iops: number, write_iops: number, await_ms: number, util: number }} DevIo
   * @typedef {{ running: number, blocked: number, iowait: number, cs: number, intr: number, swap_in: number, swap_out: number }} VmStat
   * @typedef {{ name: string, vpn: boolean, up: boolean, rx: number, tx: number, addr: string }} Iface
   * @typedef {{ free: number, available: number, buffers: number, cache: number, shared: number, dirty: number, writeback: number }} MemInfo
   * @typedef {{ pid: number, ppid: number, user: string, name: string, cmd: string, cpu: number, mem: number, swap: number, threads: number, core: number, state: string, start: number }} ProcRow
   * @typedef {{
   *   ts: number, cpu_total: number, cores: number[], core_ids?: number[],
   *   mem_total: number, mem_used: number, swap_total: number, swap_used: number,
   *   uptime: number, load1: number, load5: number, load15: number,
   *   net_rx: number, net_tx: number, procs: ProcStat[],
   *   host: string, os: string, kernel: string, cpu_name: string,
   *   disks: DiskSpace[], io: DevIo[], vm: VmStat, ifaces: Iface[],
   *   power: { profile: string, governor: string, governors: string[] },
   *   meminfo: MemInfo,
   *   bt: { present: boolean, powered: boolean, devices: string[] },
   * }} Stats
   * @typedef {{ i: number, x: number }} Hover
   */

  /** @type {Stats | null} */
  let stats = $state(null);
  /** @type {number[]} */
  let cpuHist = $state([]);
  /** @type {number[]} */
  let rxHist = $state([]);
  /** @type {number[]} */
  let txHist = $state([]);
  /** @type {number[]} */
  let rdHist = $state([]);
  /** @type {number[]} */
  let wrHist = $state([]);
  let demo = $state(false);
  let recording = $state(false);
  let recPath = $state("");
  let recErr = $state("");
  /** Loaded log for the timeline; null while showing live data. @type {Stats[] | null} */
  let log = $state.raw(null);
  let cursor = $state(0);
  /** Frozen display: live samples are dropped until resumed. */
  let paused = $state(false);
  let ctlErr = $state("");
  let busy = $state(false);
  /** Controls only make sense against the live machine. */
  const canControl = $derived(!demo && !log);

  /**
   * Runs a Tauri command, showing its error in the controls panel.
   * @param {string} name
   * @param {Record<string, unknown>} args
   */
  async function call(name, args) {
    busy = true;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const r = await invoke(name, args);
      ctlErr = "";
      return r;
    } catch (err) {
      ctlErr = String(err);
    } finally {
      busy = false;
    }
  }

  // ---------- full process list ----------
  /** @type {HTMLDialogElement | undefined} */
  let dlg = $state();
  let procsOpen = $state(false);
  /** @type {ProcRow[]} */
  let rows = $state.raw([]);
  let rowsAt = $state(0);
  let filter = $state("");
  /** @type {{ key: keyof ProcRow, desc: boolean }} */
  let sort = $state({ key: "cpu", desc: true });
  /** @type {number | null} */
  let selPid = $state(null);
  let asRoot = $state(false);
  let armKill = $state(false);
  let sigMsg = $state("");

  async function fetchRows() {
    if (demo) {
      rows = (stats?.procs ?? []).map((p) => ({ ...p, ppid: 1, user: "demo", cmd: `/usr/bin/${p.name}`, start: Date.now() / 1000 - 3600 }));
    } else {
      rows = /** @type {ProcRow[]} */ ((await call("processes", {})) ?? []);
    }
    rowsAt = Date.now();
  }

  function openProcs() {
    procsOpen = true;
    dlg?.showModal();
    fetchRows();
  }

  // Poll while the dialog is open and not paused. The backend builds the list
  // only after the first request, so an empty answer is retried soon.
  $effect(() => {
    if (!procsOpen || paused) return;
    const id = setInterval(fetchRows, rows.length ? 2000 : 1000);
    return () => clearInterval(id);
  });

  const shown = $derived.by(() => {
    const f = filter.trim().toLowerCase();
    const r = f
      ? rows.filter((p) => `${p.pid} ${p.user} ${p.name} ${p.cmd}`.toLowerCase().includes(f))
      : [...rows];
    const { key, desc } = sort;
    r.sort((a, b) => {
      const x = a[key];
      const y = b[key];
      const c = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y));
      return desc ? -c : c;
    });
    return r;
  });
  const sel = $derived(rows.find((p) => p.pid === selPid) ?? null);

  /** @param {keyof ProcRow} key */
  function sortBy(key) {
    sort = sort.key === key ? { key, desc: !sort.desc } : { key, desc: typeof rows[0]?.[key] === "number" };
  }

  /** @param {string} signal */
  async function signal(signal) {
    if (!sel) return;
    if (signal === "KILL" && !armKill) {
      armKill = true;
      return;
    }
    armKill = false;
    const target = `${sel.name} (${sel.pid})`;
    sigMsg = "";
    const before = ctlErr;
    await call("send_signal", { pid: sel.pid, signal, root: asRoot });
    sigMsg = ctlErr && ctlErr !== before ? `${signal} → ${target}: ${ctlErr}` : `${signal} sendt til ${target}`;
    ctlErr = "";
  }

  /** @param {number} start */
  function fmtAge(start) {
    const s = Math.max(0, Date.now() / 1000 - start);
    if (s < 3600) return `${Math.floor(s / 60)} m`;
    if (s < 86400) return `${Math.floor(s / 3600)} t`;
    return `${Math.floor(s / 86400)} d`;
  }

  /** @type {[keyof ProcRow, string, string][]} */
  const COLS = [
    ["pid", "pid", "r"], ["user", "bruker", ""], ["name", "navn", ""], ["cpu", "cpu %", "r"],
    ["mem", "resident", "r"], ["swap", "veksel", "r"], ["threads", "tråder", "r"], ["state", "t", ""],
    ["core", "kjerne", "r"], ["start", "startet", "r"], ["cmd", "kommando", ""],
  ];

  const PROFILES = [
    ["power-saver", "sparing"],
    ["balanced", "balansert"],
    ["performance", "ytelse"],
  ];
  /** @type {Hover | null} */
  let hovHist = $state(null);
  /** @type {Hover | null} */
  let hovNet = $state(null);
  /** @type {Hover | null} */
  let hovIo = $state(null);

  /**
   * @param {number[]} arr
   * @param {number} v
   */
  function push(arr, v) {
    const a = [...arr, v];
    if (a.length > HIST) a.shift();
    return a;
  }

  /**
   * Sum over real disks; dm volumes sit on another listed disk and would count twice.
   * @param {Stats} s
   * @param {"read_bps" | "write_bps"} k
   */
  const ioSum = (s, k) => s.io.filter((d) => !d.virt).reduce((a, d) => a + d[k], 0);

  /** @param {Stats} s */
  function ingest(s) {
    if (log || paused) return; // timeline open or paused: live samples are not shown
    stats = s;
    cpuHist = push(cpuHist, s.cpu_total);
    rxHist = push(rxHist, s.net_rx);
    txHist = push(txHist, s.net_tx);
    rdHist = push(rdHist, ioSum(s, "read_bps"));
    wrHist = push(wrHist, ioSum(s, "write_bps"));
  }

  /** Show the logged sample at index i, with the HIST samples before it as history. */
  function seek(/** @type {number} */ i) {
    if (!log) return;
    cursor = i;
    const w = log.slice(Math.max(0, i - HIST + 1), i + 1);
    stats = log[i];
    cpuHist = w.map((s) => s.cpu_total);
    rxHist = w.map((s) => s.net_rx);
    txHist = w.map((s) => s.net_tx);
    rdHist = w.map((s) => ioSum(s, "read_bps"));
    wrHist = w.map((s) => ioSum(s, "write_bps"));
  }

  /** @param {Event & { currentTarget: HTMLInputElement }} e */
  async function openLog(e) {
    const file = e.currentTarget.files?.[0];
    e.currentTarget.value = "";
    if (!file) return;
    /** @type {Stats[]} */
    const rows = [];
    for (const line of (await file.text()).split("\n")) {
      try {
        if (line.trim()) rows.push(JSON.parse(line));
      } catch {
        // a half-written last line from a crash is skipped
      }
    }
    if (!rows.length || !rows[0].ts) {
      recErr = `${file.name}: ingen målinger funnet`;
      return;
    }
    recErr = "";
    log = rows;
    seek(rows.length - 1);
  }

  function closeLog() {
    log = null;
    stats = null;
    cpuHist = rxHist = txHist = rdHist = wrHist = [];
  }

  async function toggleRec() {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      recPath = await invoke("set_recording", { on: !recording });
      recording = !recording;
      recErr = "";
    } catch (err) {
      recErr = String(err);
    }
  }

  /** Whole-log CPU outline behind the slider, max per bucket so spikes survive. */
  const overview = $derived.by(() => {
    const l = log;
    if (!l) return "";
    const n = Math.min(600, l.length);
    const per = l.length / n;
    const pts = Array.from({ length: n }, (_, b) => {
      let m = 0;
      for (let j = Math.floor(b * per); j < Math.floor((b + 1) * per); j++) m = Math.max(m, l[j].cpu_total);
      return m;
    });
    return areaPath(pts, 100, 600, 40, n);
  });

  /** @param {number} ts */
  const fmtTs = (ts) => new Date(ts).toLocaleString("nb-NO", { dateStyle: "short", timeStyle: "medium" });

  onMount(() => {
    let cleanup = () => {};
    if ("__TAURI_INTERNALS__" in window) {
      import("@tauri-apps/api/event").then(({ listen }) => {
        listen("stats", (e) => ingest(/** @type {Stats} */ (e.payload))).then((un) => (cleanup = un));
      });
    } else {
      demo = true;
      const cores = 8;
      let vals = Array.from({ length: cores }, () => 15 + Math.random() * 40);
      let rx = 2e5;
      let tx = 5e4;
      const names = ["firefox", "node", "rustc", "maskinrom", "postgres", "code"];
      const t0 = 428943;
      const tick = () => {
        vals = vals.map((v) => Math.min(100, Math.max(1, v + (Math.random() - 0.5) * 22)));
        rx = Math.max(2e3, Math.min(3e7, rx * (0.65 + Math.random() * 0.8)));
        tx = Math.max(500, Math.min(8e6, tx * (0.65 + Math.random() * 0.8)));
        const total = vals.reduce((a, b) => a + b, 0) / cores;
        ingest({
          ts: Date.now(),
          cpu_total: total,
          cores: [...vals],
          mem_total: 33421772800,
          mem_used: 33421772800 * (0.4 + total / 500),
          swap_total: 8589934592,
          swap_used: 913 * 1024 * 1024,
          uptime: t0 + Math.floor(performance.now() / 1000),
          load1: total / 25,
          load5: total / 30,
          load15: total / 35,
          net_rx: rx,
          net_tx: tx,
          procs: names.map((n, i) => ({
            pid: 1000 + i * 137,
            swap: i === 4 ? 52428800 : 0,
            threads: 1 + ((i * 7) % 40),
            core: (i * 5) % cores,
            state: i ? "S" : "R",
            name: n,
            cpu: Math.max(0.3, vals[i % cores] * (0.95 - i * 0.14)),
            mem: (6 - i) * 387973120 * (0.8 + Math.random() * 0.4),
          })),
          host: "demo-maskin",
          os: "Demo Linux",
          kernel: "6.8.0-demo",
          cpu_name: "Demodata — kjør «pnpm tauri dev» for ekte tall",
          disks: [
            { mount: "/", dev: "/dev/nvme0n1p2", fs: "ext4", total: 1e12, avail: 3.1e11 },
            { mount: "/home", dev: "/dev/nvme1n1p1", fs: "ext4", total: 2e12, avail: 1.5e11 },
            { mount: "/boot/efi", dev: "/dev/nvme0n1p1", fs: "vfat", total: 5.4e8, avail: 4.1e8 },
          ],
          io: [
            { name: "nvme0n1", virt: false, read_bps: rx * 3, write_bps: tx * 9, read_iops: rx / 4e3, write_iops: tx / 2e3, await_ms: 0.4, util: Math.min(100, total / 3) },
            { name: "nvme1n1", virt: false, read_bps: rx / 2, write_bps: tx, read_iops: rx / 2e4, write_iops: tx / 1e4, await_ms: 0.2, util: total / 12 },
          ],
          vm: { running: Math.round(total / 10), blocked: total > 80 ? 1 : 0, iowait: total / 40, cs: 2e4 + total * 300, intr: 9e3 + total * 100, swap_in: 0, swap_out: 0 },
          ifaces: [
            { name: "eno1", vpn: false, up: true, rx, tx, addr: "192.168.1.20/24" },
            { name: "wg0", vpn: true, up: true, rx: rx / 10, tx: tx / 10, addr: "10.0.0.2/24" },
          ],
          power: { profile: "balanced", governor: "powersave", governors: ["performance", "powersave"] },
          meminfo: { free: 4.2e9, available: 1.6e10, buffers: 6e8, cache: 1.1e10, shared: 7e8, dirty: 3e7, writeback: 0 },
          bt: { present: true, powered: true, devices: ["Demohøyttaler"] },
        });
      };
      tick();
      const id = setInterval(tick, 1000);
      cleanup = () => clearInterval(id);
    }
    return () => cleanup();
  });

  // ---------- formatting ----------
  /** @param {number | null | undefined} b */
  function fmtBytes(b) {
    if (b == null) return "–";
    const u = ["B", "KiB", "MiB", "GiB", "TiB"];
    let i = 0;
    let v = b;
    while (v >= 1024 && i < u.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${u[i]}`;
  }
  /** @param {number} b */
  const fmtRate = (b) => `${fmtBytes(b)}/s`;
  /** @param {number} n */
  const fmtNum = (n) => (n >= 1e4 ? `${(n / 1e3).toFixed(0)}k` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k` : n.toFixed(0));
  /** @param {number} s */
  function fmtUp(s) {
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    return `${d ? d + " d " : ""}${h} t ${m} m`;
  }

  // ---------- gauge geometry ----------
  /**
   * @param {number} cx
   * @param {number} cy
   * @param {number} r
   * @param {number} deg
   */
  function polar(cx, cy, r, deg) {
    const rad = (deg * Math.PI) / 180;
    return [cx + r * Math.cos(rad), cy - r * Math.sin(rad)];
  }
  /**
   * @param {number} cx
   * @param {number} cy
   * @param {number} r
   * @param {number} a0
   * @param {number} a1
   */
  function arc(cx, cy, r, a0, a1) {
    const [x0, y0] = polar(cx, cy, r, a0);
    const [x1, y1] = polar(cx, cy, r, a1);
    return `M${x0.toFixed(2)},${y0.toFixed(2)} A${r},${r} 0 ${a0 - a1 > 180 ? 1 : 0} 1 ${x1.toFixed(2)},${y1.toFixed(2)}`;
  }
  const GA0 = 210; // gauge sweeps 240° from 210° to −30°
  const cpuNow = $derived.by(() => (stats ? stats.cpu_total : 0));
  const gaugeVal = $derived(arc(100, 104, 78, GA0, GA0 - Math.max(0.5, Math.min(100, cpuNow)) * 2.4));
  const ticks = $derived(
    Array.from({ length: 11 }, (_, i) => {
      const a = GA0 - i * 24;
      const [x1, y1] = polar(100, 104, 84, a);
      const [x2, y2] = polar(100, 104, 90, a);
      return { x1, y1, x2, y2, warn: i >= 8 };
    })
  );

  // ---------- sparklines ----------
  const CW = 600;
  const CH = 130;
  /**
   * @param {number[]} data
   * @param {number} max
   */
  function linePath(data, max, w = CW, h = CH, slots = HIST) {
    if (data.length < 2) return "";
    const step = w / (slots - 1);
    const x0 = w - (data.length - 1) * step;
    return data
      .map((v, i) => `${i ? "L" : "M"}${(x0 + i * step).toFixed(1)},${(h - 3 - (v / max) * (h - 10)).toFixed(1)}`)
      .join("");
  }
  /**
   * @param {number[]} data
   * @param {number} max
   */
  function areaPath(data, max, w = CW, h = CH, slots = HIST) {
    if (data.length < 2) return "";
    const step = w / (slots - 1);
    const x0 = w - (data.length - 1) * step;
    return `${linePath(data, max, w, h, slots)} L${w},${h} L${x0.toFixed(1)},${h} Z`;
  }
  const netMax = $derived(Math.max(1024, ...rxHist, ...txHist) * 1.15);
  const ioMax = $derived(Math.max(1024, ...rdHist, ...wrHist) * 1.15);

  /**
   * @param {MouseEvent & { currentTarget: HTMLElement }} e
   * @param {number[]} data
   * @param {(h: Hover | null) => void} set
   */
  function chartHover(e, data, set) {
    const rect = e.currentTarget.getBoundingClientRect();
    const frac = (e.clientX - rect.left) / rect.width;
    const step = 1 / (HIST - 1);
    const first = 1 - (data.length - 1) * step;
    const i = Math.round((frac - first) / step);
    if (i >= 0 && i < data.length) set({ i, x: (first + i * step) * 100 });
    else set(null);
  }

  const memPct = $derived.by(() => (stats && stats.mem_total ? (stats.mem_used / stats.mem_total) * 100 : 0));
  const swapPct = $derived.by(() => (stats && stats.swap_total ? (stats.swap_used / stats.swap_total) * 100 : 0));
  const maxProcCpu = $derived.by(() => (stats ? Math.max(5, ...stats.procs.map((p) => p.cpu)) : 5));
</script>

<svelte:head>
  <title>Maskinrom</title>
</svelte:head>

<main>
  <header>
    <div class="brand">
      <span class="lamp" class:live={!demo && !log && !paused} class:beat={cpuHist.length % 2 === 1} aria-hidden="true"></span>
      <h1>MASKINROM</h1>
      {#if log}<span class="badge">LOGG</span>{:else if paused}<span class="badge">PAUSE</span>{:else if demo}<span class="badge">DEMO</span>{:else}<span class="badge live-badge">SANNTID</span>{/if}
    </div>
    <div class="controls">
      {#if !log}
        <button class="btn" class:paused-on={paused} onclick={() => (paused = !paused)} title="Frys visningen for å se på et øyeblikksbilde">
          {paused ? "▶ Fortsett" : "❚❚ Pause"}
        </button>
      {/if}
      {#if !demo}
        <button class="btn" class:rec-on={recording} onclick={toggleRec} title={recording ? `Skriver til ${recPath}` : "Start opptak til loggfil"}>
          <span class="rec-dot"></span>{recording ? "Stopp opptak" : "Opptak"}
        </button>
      {/if}
      {#if log}
        <button class="btn" onclick={closeLog}>Sanntid</button>
      {:else}
        <label class="btn">Åpne logg<input type="file" accept=".jsonl,application/x-ndjson,text/plain" onchange={openLog} hidden /></label>
      {/if}
    </div>
    {#if stats}
      <div class="ident">
        <span class="host">{stats.host}</span>
        <span class="sep">·</span>
        <span>{stats.os}</span>
        <span class="sep">·</span>
        <span>kjerne {stats.kernel}</span>
      </div>
      <div class="meta">
        <span class="eyebrow">Oppetid</span>
        <span class="val">{fmtUp(stats.uptime)}</span>
        <span class="eyebrow">Last</span>
        <span class="val">{stats.load1.toFixed(2)} · {stats.load5.toFixed(2)} · {stats.load15.toFixed(2)}</span>
      </div>
    {/if}
  </header>

  {#if recErr}<p class="rec-err">{recErr}</p>{/if}
  {#if recording && recPath}<p class="rec-path">● opptak til {recPath}</p>{/if}

  {#if log && stats}
    <section class="timeline" aria-label="Tidslinje">
      <div class="tl-head">
        <span class="tl-now">{fmtTs(stats.ts)}</span>
        <span class="dim">{cursor + 1} / {log.length} · {fmtTs(log[0].ts)} – {fmtTs(log[log.length - 1].ts)}</span>
      </div>
      <div class="tl-track">
        <svg viewBox="0 0 600 40" preserveAspectRatio="none" aria-hidden="true">
          <path class="area tl-area" d={overview} />
        </svg>
        <input
          type="range"
          min="0"
          max={log.length - 1}
          value={cursor}
          oninput={(e) => seek(+e.currentTarget.value)}
          aria-label="Tidspunkt i loggen"
        />
      </div>
    </section>
  {/if}

  {#if !stats}
    <p class="waiting">Venter på første måling …</p>
  {:else}
    <svelte:boundary>
    {#snippet failed(error, reset)}
      <p class="rec-err">Visningen feilet: {String(error)} <button class="btn" onclick={reset}>Prøv igjen</button></p>
    {/snippet}
    <div class="grid">
      <!-- Signatur: maskintelegrafen -->
      <section class="panel p-gauge" aria-label="Prosessorlast nå">
        <svg viewBox="0 0 200 132" role="img" aria-label="CPU-last {cpuNow.toFixed(0)} prosent">
          <path class="track" d={arc(100, 104, 78, GA0, -30)} />
          <path class="fill" d={gaugeVal} />
          {#each ticks as t}
            <line class="tick" class:warn={t.warn} x1={t.x1} y1={t.y1} x2={t.x2} y2={t.y2} />
          {/each}
          <g class="needle" style="transform: rotate({Math.min(100, cpuNow) * 2.4}deg)">
            <line x1="100" y1="104" x2={polar(100, 104, 64, GA0)[0]} y2={polar(100, 104, 64, GA0)[1]} />
          </g>
          <circle class="hub" cx="100" cy="104" r="5" />
          <text class="g-num" x="100" y="84">{cpuNow.toFixed(0)}<tspan class="g-pct">%</tspan></text>
        </svg>
        <p class="g-label">CPU-last</p>
        <p class="g-sub" title={stats.cpu_name}>{stats.cpu_name}</p>
      </section>

      <!-- CPU-historikk -->
      <section class="panel p-hist">
        <div class="p-head">
          <h2>Prosessor · siste {HIST} s</h2>
          <span class="p-val">{cpuNow.toFixed(1)} %</span>
        </div>
        <div
          class="chart"
          role="img"
          aria-label="CPU-last siste {HIST} sekunder"
          onmousemove={(e) => chartHover(e, cpuHist, (v) => (hovHist = v))}
          onmouseleave={() => (hovHist = null)}
        >
          <svg viewBox="0 0 {CW} {CH}" preserveAspectRatio="none">
            <line class="gridline" x1="0" y1={CH - 3 - 0.5 * (CH - 10)} x2={CW} y2={CH - 3 - 0.5 * (CH - 10)} />
            <path class="area amber-a" d={areaPath(cpuHist, 100)} />
            <path class="line amber-l" d={linePath(cpuHist, 100)} />
            {#if hovHist}
              <line class="cross" x1={(hovHist.x / 100) * CW} y1="0" x2={(hovHist.x / 100) * CW} y2={CH} />
            {/if}
          </svg>
          <span class="axis top">100</span><span class="axis mid">50</span>
          {#if hovHist}
            <div class="tip" style="left:{hovHist.x}%">
              <span class="tip-t">−{cpuHist.length - 1 - hovHist.i} s</span>
              <span class="tip-v">{cpuHist[hovHist.i].toFixed(1)} %</span>
            </div>
          {/if}
        </div>
      </section>

      <!-- Kjerner -->
      <section class="panel p-cores">
        <div class="p-head">
          <h2>Kjerner</h2>
          <span class="p-val">{stats.cores.length} stk</span>
        </div>
        <div class="cores">
          {#each stats.cores as c, i}
            <div class="core" title="Kjerne {stats.core_ids?.[i] ?? i}: {c.toFixed(1)} %">
              <div class="core-track"><div class="core-fill" style="height:{Math.max(2, c)}%"></div></div>
              <span class="core-n">{stats.core_ids?.[i] ?? i}</span>
            </div>
          {/each}
        </div>
      </section>

      <!-- Minne -->
      <section class="panel p-mem">
        <div class="p-head">
          <h2>Minne</h2>
          <span class="p-val">{memPct.toFixed(0)} %</span>
        </div>
        {#if stats.meminfo}
          {@const m = stats.meminfo}
          {@const used = Math.max(0, stats.mem_total - m.free - m.buffers - m.cache)}
          {@const total = stats.mem_total}
          {@const pc = (/** @type {number} */ v) => (v / total) * 100}
          <div class="meter stack" title="brukt · buffer/cache · ledig">
            <div class="meter-fill blue" style="width:{pc(used)}%"></div>
            <div class="meter-fill cache" style="width:{pc(m.buffers + m.cache)}%"></div>
          </div>
          <p class="meter-txt">{fmtBytes(stats.mem_total - m.available)} <span class="dim">i bruk av {fmtBytes(stats.mem_total)}</span></p>
          <dl class="memkv">
            <dt><span class="chip blue-c sm"></span>brukt</dt><dd>{fmtBytes(used)}</dd>
            <dt><span class="chip cache-c sm"></span>buffer/cache</dt><dd>{fmtBytes(m.buffers + m.cache)}</dd>
            <dt title="tmpfs og delt minne (Shmem)">delt</dt><dd>{fmtBytes(m.shared)}</dd>
            <dt title="Helt ubrukt (MemFree)">ledig</dt><dd>{fmtBytes(m.free)}</dd>
            <dt title="Kan tas i bruk uten å veksle (MemAvailable)">tilgjengelig</dt><dd class="hi">{fmtBytes(m.available)}</dd>
            <dt title="Endrede sider som venter på å bli skrevet til disk (Dirty / Writeback)">skitne · skrives</dt><dd>{fmtBytes(m.dirty)} · {fmtBytes(m.writeback)}</dd>
          </dl>
        {:else}
          <div class="meter">
            <div class="meter-fill blue" style="width:{memPct}%"></div>
          </div>
          <p class="meter-txt">{fmtBytes(stats.mem_used)} <span class="dim">av {fmtBytes(stats.mem_total)}</span></p>
        {/if}
        {#if stats.swap_total > 0}
          <div class="meter slim">
            <div class="meter-fill blue" style="width:{swapPct}%"></div>
          </div>
          <p class="meter-txt small">veksel {fmtBytes(stats.swap_used)} <span class="dim">av {fmtBytes(stats.swap_total)}</span></p>
        {/if}
      </section>

      <!-- Nett -->
      <section class="panel p-net">
        <div class="p-head">
          <h2>Nettverk</h2>
          <span class="p-val">
            <span class="chip blue-c"></span>ned {fmtRate(stats.net_rx)}
            <span class="chip amber-c"></span>opp {fmtRate(stats.net_tx)}
          </span>
        </div>
        <div
          class="chart"
          role="img"
          aria-label="Nettverkstrafikk siste {HIST} sekunder"
          onmousemove={(e) => chartHover(e, rxHist, (v) => (hovNet = v))}
          onmouseleave={() => (hovNet = null)}
        >
          <svg viewBox="0 0 {CW} {CH}" preserveAspectRatio="none">
            <path class="line blue-l" d={linePath(rxHist, netMax)} />
            <path class="line amber-l thin" d={linePath(txHist, netMax)} />
            {#if hovNet}
              <line class="cross" x1={(hovNet.x / 100) * CW} y1="0" x2={(hovNet.x / 100) * CW} y2={CH} />
            {/if}
          </svg>
          {#if hovNet}
            <div class="tip" style="left:{hovNet.x}%">
              <span class="tip-t">−{rxHist.length - 1 - hovNet.i} s</span>
              <span class="tip-v">↓ {fmtRate(rxHist[hovNet.i])}</span>
              <span class="tip-v">↑ {fmtRate(txHist[hovNet.i] ?? 0)}</span>
            </div>
          {/if}
        </div>
        <div class="ifaces">
          {#each stats.ifaces as i}
            <span class="if-n"><span class="dot" class:on-dot={i.up}></span>{i.name}{#if i.vpn}<span class="vpn">VPN</span>{/if}</span>
            <span class="dim">{i.addr || (i.up ? "" : "nede")}</span>
            <span class="r">↓ {fmtRate(i.rx)}</span>
            <span class="r">↑ {fmtRate(i.tx)}</span>
          {/each}
        </div>
      </section>

      <!-- Diskplass -->
      <section class="panel p-space">
        <div class="p-head">
          <h2>Diskplass</h2>
          <span class="p-val">ledig · brukt</span>
        </div>
        {#each stats.disks as d}
          {@const pct = d.total ? ((d.total - d.avail) / d.total) * 100 : 0}
          <div class="space" title="{d.mount} · {d.dev} · {d.fs} · {fmtBytes(d.avail)} ledig av {fmtBytes(d.total)}">
            <p class="meter-txt small">
              <span class="mount">{d.mount}</span>
              <span class="dim">{fmtBytes(d.avail)}</span>
              <span class="pct" class:crit={pct >= 90}>{pct.toFixed(0)} %</span>
            </p>
            <div class="meter slim">
              <div class="meter-fill blue" class:crit-fill={pct >= 90} style="width:{pct}%"></div>
            </div>
          </div>
        {/each}
      </section>

      <!-- Disk-I/O -->
      <section class="panel p-io">
        <div class="p-head">
          <h2>Disk-I/O</h2>
          <span class="p-val">
            <span class="chip blue-c"></span>les {fmtRate(rdHist.at(-1) ?? 0)}
            <span class="chip amber-c"></span>skriv {fmtRate(wrHist.at(-1) ?? 0)}
          </span>
        </div>
        <div
          class="chart"
          role="img"
          aria-label="Disk-I/O siste {HIST} sekunder"
          onmousemove={(e) => chartHover(e, rdHist, (v) => (hovIo = v))}
          onmouseleave={() => (hovIo = null)}
        >
          <svg viewBox="0 0 {CW} {CH}" preserveAspectRatio="none">
            <path class="line blue-l" d={linePath(rdHist, ioMax)} />
            <path class="line amber-l thin" d={linePath(wrHist, ioMax)} />
            {#if hovIo}
              <line class="cross" x1={(hovIo.x / 100) * CW} y1="0" x2={(hovIo.x / 100) * CW} y2={CH} />
            {/if}
          </svg>
          {#if hovIo}
            <div class="tip" style="left:{hovIo.x}%">
              <span class="tip-t">−{rdHist.length - 1 - hovIo.i} s</span>
              <span class="tip-v">les {fmtRate(rdHist[hovIo.i])}</span>
              <span class="tip-v">skriv {fmtRate(wrHist[hovIo.i] ?? 0)}</span>
            </div>
          {/if}
        </div>
        <div class="devs">
          <span class="eyebrow">enhet</span><span class="eyebrow r">les</span><span class="eyebrow r">skriv</span>
          <span class="eyebrow r">iops</span><span class="eyebrow r" title="Snitt ventetid per forespørsel (await)">vent</span>
          <span class="eyebrow" title="Andel av tiden disken har I/O i gang (%util). NVMe kan gjøre mye parallelt, så 100 % er ikke nødvendigvis fullt.">opptatt</span>
          {#each stats.io as d}
            <span class="dev-n">{d.name}</span>
            <span class="r">{fmtRate(d.read_bps)}</span>
            <span class="r">{fmtRate(d.write_bps)}</span>
            <span class="r">{fmtNum(d.read_iops + d.write_iops)}</span>
            <span class="r">{d.await_ms.toFixed(1)} ms</span>
            <span class="util">
              <span class="pr-bar"><span class="pr-fill" class:crit-fill={d.util >= 90} style="width:{d.util}%"></span></span>
              <span class="r">{d.util.toFixed(0)} %</span>
            </span>
          {/each}
        </div>
      </section>

      <!-- vmstat -->
      <section class="panel p-vm">
        <div class="p-head">
          <h2>Kjerne · vmstat</h2>
          <span class="p-val">per sekund</span>
        </div>
        <div class="tiles">
          <div class="tile" title="Prosesser som kjører eller venter på CPU (r)">
            <span class="eyebrow">kjørbare</span><span class="t-val">{stats.vm.running}</span>
          </div>
          <div class="tile" title="Prosesser blokkert i ventende I/O, tilstand D (b)">
            <span class="eyebrow">blokkert</span><span class="t-val" class:crit={stats.vm.blocked > 0}>{stats.vm.blocked}</span>
          </div>
          <div class="tile" title="Andel CPU-tid ledig mens den venter på I/O (wa)">
            <span class="eyebrow">i/o-vent</span><span class="t-val" class:crit={stats.vm.iowait >= 10}>{stats.vm.iowait.toFixed(1)} %</span>
          </div>
          <div class="tile" title="Kontekstbytter per sekund (cs)">
            <span class="eyebrow">kontekstbytter</span><span class="t-val">{fmtNum(stats.vm.cs)}</span>
          </div>
          <div class="tile" title="Avbrudd per sekund (in)">
            <span class="eyebrow">avbrudd</span><span class="t-val">{fmtNum(stats.vm.intr)}</span>
          </div>
          <div class="tile" title="Sider vekslet inn / ut per sekund (si / so). Vedvarende ut-veksling betyr minnemangel.">
            <span class="eyebrow">veksel inn/ut</span>
            <span class="t-val" class:crit={stats.vm.swap_in + stats.vm.swap_out > 0}>{fmtNum(stats.vm.swap_in)} / {fmtNum(stats.vm.swap_out)}</span>
          </div>
        </div>
      </section>

      <!-- Strøm og Bluetooth -->
      <section class="panel p-dev">
        <div class="p-head">
          <h2>Strøm · Bluetooth</h2>
        </div>
        <dl class="kv">
          <dt class="eyebrow">strømmodus</dt>
          <dd class="seg">
            {#each PROFILES as [id, label]}
              <button class="seg-b" class:seg-on={stats.power.profile === id} disabled={!canControl || busy}
                onclick={() => call("set_power_profile", { profile: id })} title={id}>{label}</button>
            {/each}
          </dd>
          <dt class="eyebrow" title="Krever root, du får spørsmål om passord">cpu-governor</dt>
          <dd class="seg">
            {#each stats.power.governors ?? [stats.power.governor] as g}
              <button class="seg-b" class:seg-on={stats.power.governor === g} disabled={!canControl || busy}
                onclick={() => call("set_governor", { governor: g })}>{g}</button>
            {/each}
          </dd>
          <dt class="eyebrow">bluetooth</dt>
          <dd>
            {#if !stats.bt.present}<span class="dim">ingen adapter</span>
            {:else}
              <button class="switch" class:sw-on={stats.bt.powered} role="switch" aria-checked={stats.bt.powered}
                aria-label="Bluetooth" disabled={!canControl || busy}
                onclick={() => call("set_bluetooth", { on: !stats?.bt.powered })}><span></span></button>
              <span class:on={stats.bt.powered} class:dim={!stats.bt.powered}>{stats.bt.powered ? "på" : "av"}</span>
            {/if}
          </dd>
          {#each stats.bt.devices as name}
            <dt></dt>
            <dd class="bt-dev">{name}</dd>
          {/each}
        </dl>
        {#if ctlErr}<p class="ctl-err">{ctlErr}</p>{/if}
      </section>

      <!-- Prosesser -->
      <section class="panel p-proc">
        <div class="p-head">
          <h2>Travleste prosesser</h2>
          <span class="p-val">
            cpu = prosent av én kjerne
            <button class="btn sm" onclick={openProcs} disabled={!!log} title={log ? "Loggen har bare de seks travleste" : "Vis alle prosesser og send signaler"}>Alle prosesser…</button>
          </span>
        </div>
        <ol class="procs">
          <li class="pr-head">
            <span class="eyebrow">navn</span>
            <span class="eyebrow r">pid</span>
            <span></span>
            <span class="eyebrow r">cpu</span>
            <span class="eyebrow r" title="Kjernen prosessen sist kjørte på">kjerne</span>
            <span class="eyebrow r" title="Tilstand: R kjører, S sover, D venter på I/O, Z zombie">t</span>
            <span class="eyebrow r" title="Tråder">tråder</span>
            <span class="eyebrow r" title="Resident minne (RSS)">resident</span>
            <span class="eyebrow r" title="Minne vekslet ut til swap">veksel</span>
          </li>
          {#each stats.procs as p}
            <li>
              <span class="pr-name">{p.name}</span>
              <span class="pr-mem">{p.pid}</span>
              <span class="pr-bar"><span class="pr-fill" style="width:{(p.cpu / maxProcCpu) * 100}%"></span></span>
              <span class="pr-cpu">{p.cpu.toFixed(1)} %</span>
              <span class="pr-cpu">{p.core >= 0 ? p.core : "–"}</span>
              <span class="pr-cpu" class:crit={p.state === "D"}>{p.state || "–"}</span>
              <span class="pr-mem">{p.threads || "–"}</span>
              <span class="pr-cpu">{fmtBytes(p.mem)}</span>
              <span class="pr-mem" class:crit={p.swap > 0}>{p.swap ? fmtBytes(p.swap) : "–"}</span>
            </li>
          {/each}
        </ol>
      </section>
    </div>
    </svelte:boundary>
  {/if}
</main>

<dialog bind:this={dlg} class="procdlg" onclose={() => { procsOpen = false; armKill = false; }}>
  <div class="pd-head">
    <h2>Alle prosesser</h2>
    <span class="dim">{shown.length} av {rows.length} · {paused ? "pauset" : "oppdateres hvert 2. sekund"}{rowsAt ? ` · ${new Date(rowsAt).toLocaleTimeString("nb-NO")}` : ""}</span>
    <input class="pd-filter" type="search" placeholder="Filtrer på navn, kommando, bruker, pid" bind:value={filter} />
    <button class="btn" class:paused-on={paused} onclick={() => (paused = !paused)}>{paused ? "▶ Fortsett" : "❚❚ Pause"}</button>
    <button class="btn" onclick={() => dlg?.close()}>Lukk</button>
  </div>
  <div class="pd-table">
    <table>
      <thead>
        <tr>
          {#each COLS as [key, label, cls]}
            <th class={cls}><button onclick={() => sortBy(key)}>{label}{sort.key === key ? (sort.desc ? " ▾" : " ▴") : ""}</button></th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each shown as p (p.pid)}
          <tr class:sel={p.pid === selPid} onclick={() => { selPid = p.pid; armKill = false; }}>
            <td class="r">{p.pid}</td>
            <td>{p.user}</td>
            <td class="nm">{p.name}</td>
            <td class="r">{p.cpu.toFixed(1)}</td>
            <td class="r">{fmtBytes(p.mem)}</td>
            <td class="r" class:crit={p.swap > 0}>{p.swap ? fmtBytes(p.swap) : "–"}</td>
            <td class="r">{p.threads || "–"}</td>
            <td class:crit={p.state === "D" || p.state === "Z"} class:dim={p.state === "T"}>{p.state}</td>
            <td class="r">{p.core >= 0 ? p.core : "–"}</td>
            <td class="r" title={new Date(p.start * 1000).toLocaleString("nb-NO")}>{fmtAge(p.start)}</td>
            <td class="cmd" title={p.cmd}>{p.cmd}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <div class="pd-act">
    {#if sel}
      <span class="pd-sel">{sel.name} <span class="dim">pid {sel.pid} · {sel.user}</span></span>
      {#each [["TERM", "Avslutt (TERM)"], ["INT", "Avbryt (INT)"], ["HUP", "HUP"], ["STOP", "Frys (STOP)"], ["CONT", "Fortsett (CONT)"]] as [sig, label]}
        <button class="btn" disabled={busy || demo} onclick={() => signal(sig)}>{label}</button>
      {/each}
      <button class="btn danger" class:armed={armKill} disabled={busy || demo} onclick={() => signal("KILL")}>{armKill ? "Sikker? Klikk igjen" : "Drep (KILL)"}</button>
      <label class="pd-root"><input type="checkbox" bind:checked={asRoot} /> som root</label>
    {:else}
      <span class="dim">Velg en prosess for å sende signal. Trykk Pause for å holde lista i ro.</span>
    {/if}
    {#if sigMsg}<span class="pd-msg">{sigMsg}</span>{/if}
  </div>
</dialog>

<style>
  :global(:root) {
    --bg: #0b0f16;
    --panel: #10161f;
    --hairline: #1c2534;
    --ink: #e8edf4;
    --ink-2: #8b98a9;
    --ink-3: #5a6678;
    --amber: #ffb454;
    --amber-deep: #c98420;
    --blue: #6dc5f2;
    --blue-deep: #2e8fc7;
    --crit: #f0716d;
    --font-display: "DM Mono", "Fira Code", ui-monospace, monospace;
    --font-data: "Fira Code", ui-monospace, monospace;
    --font-label: "Roboto Condensed", "Ubuntu", sans-serif;
  }
  :global(html) {
    background: var(--bg);
  }
  :global(body) {
    margin: 0;
    background: var(--bg);
    color: var(--ink);
    font-family: var(--font-data);
    font-size: 13px;
  }

  main {
    max-width: 980px;
    margin: 0 auto;
    padding: 14px 16px 20px;
  }

  /* ---------- header ---------- */
  header {
    display: flex;
    align-items: baseline;
    gap: 14px;
    flex-wrap: wrap;
    padding: 4px 2px 12px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  h1 {
    font-family: var(--font-display);
    font-weight: 500;
    font-size: 17px;
    letter-spacing: 0.32em;
    margin: 0;
  }
  .lamp {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--amber);
  }
  /* Blinks once per sample instead of a looping animation: an endless CSS
     animation kept WebKit repainting at 60 fps (~50 % CPU). */
  .lamp.live.beat {
    opacity: 0.3;
  }
  .badge {
    font-family: var(--font-label);
    font-size: 10px;
    letter-spacing: 0.14em;
    color: var(--bg);
    background: var(--amber);
    padding: 2px 7px;
    border-radius: 3px;
  }
  .badge.live-badge {
    background: transparent;
    color: var(--amber);
    border: 1px solid var(--amber-deep);
  }
  .ident {
    color: var(--ink-2);
    font-size: 12px;
  }
  .ident .host {
    color: var(--ink);
  }
  .sep {
    color: var(--ink-3);
    margin: 0 4px;
  }
  .meta {
    margin-left: auto;
    display: flex;
    gap: 8px;
    align-items: baseline;
    font-size: 12px;
  }
  .eyebrow {
    font-family: var(--font-label);
    text-transform: uppercase;
    letter-spacing: 0.13em;
    font-size: 10px;
    color: var(--ink-3);
  }
  .meta .val {
    color: var(--ink-2);
    margin-right: 8px;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .btn.sm {
    padding: 1px 7px;
    margin-left: 10px;
    font-size: 10px;
  }
  .btn.paused-on {
    color: var(--amber);
    border-color: var(--amber-deep);
  }
  .btn.danger {
    color: var(--crit);
  }
  .btn.danger.armed {
    background: var(--crit);
    color: var(--bg);
  }
  .controls {
    display: flex;
    gap: 6px;
  }
  .btn {
    font-family: var(--font-label);
    font-size: 11px;
    letter-spacing: 0.08em;
    color: var(--ink-2);
    background: var(--panel);
    border: 1px solid var(--hairline);
    border-radius: 4px;
    padding: 3px 9px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .btn:hover {
    color: var(--ink);
    border-color: var(--ink-3);
  }
  .rec-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ink-3);
  }
  .btn.rec-on {
    color: var(--crit);
    border-color: var(--crit);
  }
  .btn.rec-on .rec-dot {
    background: var(--crit);
  }
  .rec-err,
  .rec-path {
    font-size: 11px;
    margin: -6px 2px 10px;
    color: var(--ink-3);
  }
  .rec-err {
    color: var(--crit);
  }
  .timeline {
    background: var(--panel);
    border: 1px solid var(--hairline);
    border-radius: 7px;
    padding: 6px 14px 4px;
    margin-bottom: 10px;
  }
  .tl-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12px;
    margin-bottom: 2px;
    flex-wrap: wrap;
  }
  .tl-now {
    color: var(--amber);
  }
  .tl-track {
    position: relative;
    height: 28px;
  }
  .tl-track svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  .tl-area {
    fill: rgba(255, 180, 84, 0.22);
  }
  .tl-track input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    background: transparent;
    accent-color: var(--amber);
    cursor: pointer;
  }
  .waiting {
    color: var(--ink-2);
    padding: 40px 0;
    text-align: center;
  }

  /* ---------- grid & panels ---------- */
  .grid {
    display: grid;
    grid-template-columns: repeat(12, 1fr);
    gap: 10px;
  }
  .panel {
    background: var(--panel);
    border: 1px solid var(--hairline);
    border-radius: 7px;
    padding: 12px 14px;
    animation: rise 0.45s ease both;
  }
  .panel:nth-child(2) { animation-delay: 0.05s; }
  .panel:nth-child(3) { animation-delay: 0.1s; }
  .panel:nth-child(4) { animation-delay: 0.15s; }
  .panel:nth-child(5) { animation-delay: 0.2s; }
  .panel:nth-child(6) { animation-delay: 0.25s; }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
  .p-gauge { grid-column: span 4; text-align: center; }
  .p-hist { grid-column: span 8; }
  .p-cores { grid-column: span 12; }
  .p-mem { grid-column: span 4; }
  .p-net { grid-column: span 8; }
  .p-hist, .p-net {
    display: flex;
    flex-direction: column;
  }
  .p-hist .chart, .p-net .chart {
    flex: 1;
    min-height: 110px;
  }
  .p-space { grid-column: span 4; }
  .p-io { grid-column: span 8; }
  .p-vm { grid-column: span 8; }
  .p-dev { grid-column: span 4; }
  .p-io {
    display: flex;
    flex-direction: column;
  }
  .p-io .chart {
    min-height: 90px;
  }
  .p-io .chart svg {
    min-height: 90px;
  }
  .p-proc { grid-column: span 12; }
  @media (max-width: 700px) {
    .p-gauge, .p-hist, .p-mem, .p-net, .p-space, .p-io, .p-vm, .p-dev { grid-column: span 12; }
  }
  /* Wide window: three rows so everything fits on one screen. */
  @media (min-width: 1200px) {
    main {
      max-width: none;
      min-height: 100vh;
      box-sizing: border-box;
      display: flex;
      flex-direction: column;
    }
    .grid {
      flex: 1;
      grid-template-rows: repeat(3, minmax(min-content, 1fr));
      grid-template-columns: repeat(6, minmax(0, 1fr));
      grid-template-areas:
        "gauge hist hist cores cores cores"
        "mem   net  net  io    io    space"
        "vm    vm   dev  proc  proc  proc";
    }
    .p-gauge { grid-area: gauge; }
    .p-hist { grid-area: hist; }
    .p-cores { grid-area: cores; display: flex; flex-direction: column; }
    .p-cores .cores { flex: 1; }
    .p-cores .core-track { height: auto; flex: 1; min-height: 58px; }
    .p-cores .core { align-self: stretch; }
    .p-mem { grid-area: mem; }
    .p-net { grid-area: net; }
    .p-io { grid-area: io; }
    .p-io .chart { flex: 1; }
    .p-space { grid-area: space; }
    .p-vm { grid-area: vm; }
    .p-dev { grid-area: dev; }
    .p-proc { grid-area: proc; }
    .cores { gap: 4px; }
  }

  /* ---------- disks, interfaces, vmstat ---------- */
  .r {
    text-align: right;
  }
  .crit {
    color: var(--crit);
  }
  .meter-fill.crit-fill,
  .pr-fill.crit-fill {
    background: var(--crit);
  }
  .space {
    margin-bottom: 7px;
  }
  .space .meter.slim {
    margin-top: 0;
    height: 5px;
  }
  .space .meter-txt {
    display: flex;
    white-space: nowrap;
    gap: 8px;
    margin: 0 0 4px;
  }
  .space .mount {
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .space .pct {
    margin-left: auto;
    color: var(--ink-2);
  }
  .devs,
  .ifaces {
    display: grid;
    gap: 3px 12px;
    margin-top: 10px;
    font-size: 11px;
    color: var(--ink-2);
    align-items: center;
  }
  .devs {
    grid-template-columns: minmax(60px, 1fr) repeat(2, auto) auto auto minmax(90px, 1.3fr);
  }
  .dev-n,
  .if-n {
    color: var(--ink);
  }
  .util {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .util .pr-bar {
    flex: 1;
  }
  .ifaces {
    grid-template-columns: auto 1fr auto auto;
  }
  .dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ink-3);
    margin-right: 6px;
    vertical-align: middle;
  }
  .dot.on-dot {
    background: var(--blue);
  }
  .vpn {
    font-family: var(--font-label);
    font-size: 9px;
    letter-spacing: 0.12em;
    color: var(--amber);
    border: 1px solid var(--amber-deep);
    border-radius: 3px;
    padding: 0 4px;
    margin-left: 6px;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 3px;
    background: var(--bg);
    border: 1px solid var(--hairline);
    border-radius: 5px;
    padding: 7px 10px;
  }
  .t-val {
    font-family: var(--font-display);
    font-size: 17px;
    color: var(--ink);
  }
  .t-val.crit {
    color: var(--crit);
  }
  .kv {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 7px 12px;
    margin: 0;
    align-items: baseline;
  }
  .kv dd {
    margin: 0;
    color: var(--ink);
  }
  .on {
    color: var(--blue);
  }
  .seg {
    display: flex;
    flex-wrap: wrap;
    gap: 3px;
  }
  .seg-b {
    font-family: var(--font-data);
    font-size: 11px;
    color: var(--ink-2);
    background: var(--bg);
    border: 1px solid var(--hairline);
    border-radius: 4px;
    padding: 2px 7px;
    cursor: pointer;
  }
  .seg-b:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--ink-3);
  }
  .seg-b.seg-on {
    color: var(--bg);
    background: var(--amber);
    border-color: var(--amber);
  }
  .seg-b:disabled:not(.seg-on) {
    opacity: 0.5;
    cursor: default;
  }
  .switch {
    width: 30px;
    height: 16px;
    border-radius: 8px;
    border: 1px solid var(--hairline);
    background: var(--bg);
    padding: 0;
    position: relative;
    cursor: pointer;
    vertical-align: middle;
    margin-right: 6px;
  }
  .switch span {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--ink-3);
    transition: left 0.2s ease;
  }
  .switch.sw-on {
    background: var(--blue-deep);
    border-color: var(--blue-deep);
  }
  .switch.sw-on span {
    left: 16px;
    background: var(--ink);
  }
  .switch:disabled {
    cursor: default;
    opacity: 0.6;
  }
  .ctl-err {
    color: var(--crit);
    font-size: 11px;
    margin: 8px 0 0;
    word-break: break-word;
  }
  .meter.stack {
    display: flex;
  }
  .meter.stack .meter-fill {
    border-radius: 0;
  }
  .meter-fill.cache {
    background: rgba(109, 197, 242, 0.28);
  }
  .chip.cache-c {
    background: rgba(109, 197, 242, 0.4);
  }
  .chip.sm {
    width: 7px;
    height: 7px;
    margin: 0 6px 0 0;
  }
  .memkv {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 3px 10px;
    margin: 10px 0 4px;
    font-size: 11px;
  }
  .memkv dt {
    color: var(--ink-3);
  }
  .memkv dd {
    margin: 0;
    text-align: right;
    color: var(--ink-2);
  }
  .memkv dd.hi {
    color: var(--ink);
  }

  /* ---------- process dialog ---------- */
  .procdlg {
    width: min(1400px, 94vw);
    height: 86vh;
    padding: 0;
    background: var(--panel);
    color: var(--ink);
    border: 1px solid var(--hairline);
    border-radius: 8px;
    font-family: var(--font-data);
    font-size: 12px;
  }
  .procdlg[open] {
    display: flex;
    flex-direction: column;
  }
  .procdlg::backdrop {
    background: rgba(5, 8, 12, 0.7);
  }
  .pd-head,
  .pd-act {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    flex-wrap: wrap;
  }
  .pd-head {
    border-bottom: 1px solid var(--hairline);
  }
  .pd-filter {
    flex: 1;
    min-width: 200px;
    background: var(--bg);
    color: var(--ink);
    border: 1px solid var(--hairline);
    border-radius: 4px;
    padding: 4px 8px;
    font: inherit;
  }
  .pd-table {
    flex: 1;
    overflow: auto;
  }
  .pd-table table {
    width: 100%;
    border-collapse: collapse;
  }
  .pd-table th {
    position: sticky;
    top: 0;
    background: var(--panel);
    text-align: left;
    padding: 0;
    border-bottom: 1px solid var(--hairline);
  }
  .pd-table th button {
    font-family: var(--font-label);
    text-transform: uppercase;
    letter-spacing: 0.1em;
    font-size: 10px;
    color: var(--ink-3);
    background: none;
    border: 0;
    padding: 6px 8px;
    width: 100%;
    text-align: inherit;
    cursor: pointer;
  }
  .pd-table th.r {
    text-align: right;
  }
  .pd-table th button:hover {
    color: var(--ink);
  }
  .pd-table td {
    padding: 2px 8px;
    white-space: nowrap;
    color: var(--ink-2);
  }
  .pd-table td.nm {
    color: var(--ink);
  }
  .pd-table td.cmd {
    max-width: 420px;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--ink-3);
  }
  .pd-table tbody tr {
    cursor: pointer;
  }
  .pd-table tbody tr:hover {
    background: var(--bg);
  }
  .pd-table tr.sel {
    background: rgba(255, 180, 84, 0.14) !important;
  }
  .pd-act {
    border-top: 1px solid var(--hairline);
    min-height: 32px;
  }
  .pd-sel {
    color: var(--amber);
    margin-right: 6px;
  }
  .pd-root {
    color: var(--ink-2);
    font-size: 11px;
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .pd-msg {
    color: var(--ink-2);
    font-size: 11px;
    flex-basis: 100%;
  }
  .bt-dev {
    color: var(--ink-2) !important;
    font-size: 12px;
  }

  .p-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 8px;
  }
  h2 {
    font-family: var(--font-label);
    text-transform: uppercase;
    letter-spacing: 0.15em;
    font-size: 11px;
    font-weight: 500;
    color: var(--ink-2);
    margin: 0;
  }
  .p-val {
    font-size: 12px;
    color: var(--ink-2);
  }

  /* ---------- gauge ---------- */
  .p-gauge svg {
    width: 100%;
    max-width: 230px;
  }
  .track {
    fill: none;
    stroke: var(--hairline);
    stroke-width: 7;
    stroke-linecap: round;
  }
  .fill {
    fill: none;
    stroke: var(--amber);
    stroke-width: 7;
    stroke-linecap: round;
    filter: drop-shadow(0 0 5px rgba(255, 180, 84, 0.45));
  }
  .tick {
    stroke: var(--ink-3);
    stroke-width: 1.4;
  }
  .tick.warn {
    stroke: var(--crit);
  }
  .needle {
    transform-origin: 100px 104px;
    transition: transform 0.7s cubic-bezier(0.34, 1.3, 0.5, 1);
  }
  .needle line {
    stroke: var(--ink);
    stroke-width: 2;
    stroke-linecap: round;
  }
  .hub {
    fill: var(--amber);
  }
  .g-num {
    font-family: var(--font-display);
    font-size: 34px;
    fill: var(--ink);
    text-anchor: middle;
  }
  .g-pct {
    font-size: 15px;
    fill: var(--ink-2);
  }
  .g-label {
    font-family: var(--font-label);
    text-transform: uppercase;
    letter-spacing: 0.18em;
    font-size: 11px;
    color: var(--ink-2);
    margin: 2px 0 4px;
  }
  .g-sub {
    color: var(--ink-3);
    font-size: 11px;
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* ---------- charts ---------- */
  .chart {
    position: relative;
  }
  .chart svg {
    display: block;
    width: 100%;
    height: 100%;
    min-height: 110px;
  }
  .gridline {
    stroke: var(--hairline);
    stroke-width: 1;
    stroke-dasharray: 3 5;
  }
  .line {
    fill: none;
    stroke-width: 2;
    vector-effect: non-scaling-stroke;
    stroke-linejoin: round;
  }
  .amber-l {
    stroke: var(--amber);
  }
  .blue-l {
    stroke: var(--blue-deep);
  }
  .line.thin {
    stroke-width: 1.5;
    stroke: var(--amber-deep);
  }
  .area.amber-a {
    fill: rgba(255, 180, 84, 0.09);
  }
  .cross {
    stroke: var(--ink-3);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .axis {
    position: absolute;
    right: 4px;
    font-size: 9px;
    color: var(--ink-3);
    pointer-events: none;
  }
  .axis.top { top: 0; }
  .axis.mid { top: 46%; }
  .tip {
    position: absolute;
    top: 4px;
    transform: translateX(-50%);
    background: #0d1219ee;
    border: 1px solid var(--hairline);
    border-radius: 4px;
    padding: 4px 8px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    pointer-events: none;
    white-space: nowrap;
  }
  .tip-t {
    font-size: 10px;
    color: var(--ink-3);
  }
  .tip-v {
    font-size: 11px;
    color: var(--ink);
  }
  .chip {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 2px;
    margin: 0 4px 0 10px;
  }
  .chip.blue-c { background: var(--blue-deep); }
  .chip.amber-c { background: var(--amber-deep); }

  /* ---------- cores ---------- */
  .cores {
    display: flex;
    gap: 8px;
    align-items: flex-end;
  }
  .core {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }
  .core-track {
    width: 100%;
    max-width: 34px;
    height: 58px;
    background: var(--bg);
    border: 1px solid var(--hairline);
    border-radius: 4px;
    display: flex;
    align-items: flex-end;
    overflow: hidden;
  }
  .core-fill {
    width: 100%;
    background: linear-gradient(to top, var(--amber-deep), var(--amber));
    border-radius: 3px 3px 0 0;
  }
  .core-n {
    font-size: 10px;
    color: var(--ink-3);
  }

  /* ---------- meters ---------- */
  .meter {
    height: 12px;
    background: var(--bg);
    border: 1px solid var(--hairline);
    border-radius: 4px;
    overflow: hidden;
    margin-top: 6px;
  }
  .meter.slim {
    height: 6px;
    margin-top: 12px;
  }
  .meter-fill {
    height: 100%;
    border-radius: 3px;
  }
  .meter-fill.blue {
    background: linear-gradient(to right, var(--blue-deep), var(--blue));
  }
  .meter-txt {
    margin: 6px 0 0;
    font-size: 12px;
  }
  .meter-txt.small {
    font-size: 11px;
    color: var(--ink-2);
  }
  .dim {
    color: var(--ink-3);
  }

  /* ---------- processes ---------- */
  .procs {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .procs li {
    display: grid;
    grid-template-columns: minmax(90px, 160px) 56px 1fr 60px 44px 18px 48px 72px 72px;
    gap: 10px;
    align-items: center;
  }
  .pr-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pr-bar {
    height: 5px;
    background: var(--bg);
    border-radius: 3px;
    overflow: hidden;
  }
  .pr-fill {
    display: block;
    height: 100%;
    background: var(--amber-deep);
    border-radius: 3px;
  }
  .pr-cpu {
    text-align: right;
    color: var(--ink-2);
    font-size: 12px;
  }
  .pr-mem {
    text-align: right;
    color: var(--ink-3);
    font-size: 12px;
  }

  @media (prefers-reduced-motion: reduce) {
    .panel {
      animation: none;
    }
    .needle {
      transition: none;
    }
  }
</style>
