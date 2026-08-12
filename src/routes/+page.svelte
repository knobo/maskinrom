<script>
  import { onMount } from "svelte";

  const HIST = 90;

  /**
   * @typedef {{ name: string, cpu: number, mem: number }} ProcStat
   * @typedef {{
   *   cpu_total: number, cores: number[],
   *   mem_total: number, mem_used: number, swap_total: number, swap_used: number,
   *   uptime: number, load1: number, load5: number, load15: number,
   *   net_rx: number, net_tx: number, procs: ProcStat[],
   *   host: string, os: string, kernel: string, cpu_name: string,
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
  let demo = $state(false);
  /** @type {Hover | null} */
  let hovHist = $state(null);
  /** @type {Hover | null} */
  let hovNet = $state(null);

  /**
   * @param {number[]} arr
   * @param {number} v
   */
  function push(arr, v) {
    const a = [...arr, v];
    if (a.length > HIST) a.shift();
    return a;
  }

  /** @param {Stats} s */
  function ingest(s) {
    stats = s;
    cpuHist = push(cpuHist, s.cpu_total);
    rxHist = push(rxHist, s.net_rx);
    txHist = push(txHist, s.net_tx);
  }

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
            name: n,
            cpu: Math.max(0.3, vals[i % cores] * (0.95 - i * 0.14)),
            mem: (6 - i) * 387973120 * (0.8 + Math.random() * 0.4),
          })),
          host: "demo-maskin",
          os: "Demo Linux",
          kernel: "6.8.0-demo",
          cpu_name: "Demodata — kjør «pnpm tauri dev» for ekte tall",
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
  function linePath(data, max, w = CW, h = CH) {
    if (data.length < 2) return "";
    const step = w / (HIST - 1);
    const x0 = w - (data.length - 1) * step;
    return data
      .map((v, i) => `${i ? "L" : "M"}${(x0 + i * step).toFixed(1)},${(h - 3 - (v / max) * (h - 10)).toFixed(1)}`)
      .join("");
  }
  /**
   * @param {number[]} data
   * @param {number} max
   */
  function areaPath(data, max, w = CW, h = CH) {
    if (data.length < 2) return "";
    const step = w / (HIST - 1);
    const x0 = w - (data.length - 1) * step;
    return `${linePath(data, max, w, h)} L${w},${h} L${x0.toFixed(1)},${h} Z`;
  }
  const netMax = $derived(Math.max(1024, ...rxHist, ...txHist) * 1.15);

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
      <span class="lamp" class:live={!demo} aria-hidden="true"></span>
      <h1>MASKINROM</h1>
      {#if demo}<span class="badge">DEMO</span>{:else}<span class="badge live-badge">SANNTID</span>{/if}
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

  {#if !stats}
    <p class="waiting">Venter på første måling …</p>
  {:else}
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
            <div class="core" title="Kjerne {i}: {c.toFixed(1)} %">
              <div class="core-track"><div class="core-fill" style="height:{Math.max(2, c)}%"></div></div>
              {#if stats.cores.length <= 16}<span class="core-n">{c.toFixed(0)}</span>{/if}
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
        <div class="meter">
          <div class="meter-fill blue" style="width:{memPct}%"></div>
        </div>
        <p class="meter-txt">{fmtBytes(stats.mem_used)} <span class="dim">av {fmtBytes(stats.mem_total)}</span></p>
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
      </section>

      <!-- Prosesser -->
      <section class="panel p-proc">
        <div class="p-head">
          <h2>Travleste prosesser</h2>
          <span class="p-val">cpu · minne</span>
        </div>
        <ol class="procs">
          {#each stats.procs as p (p.name)}
            <li>
              <span class="pr-name">{p.name}</span>
              <span class="pr-bar"><span class="pr-fill" style="width:{(p.cpu / maxProcCpu) * 100}%"></span></span>
              <span class="pr-cpu">{p.cpu.toFixed(1)} %</span>
              <span class="pr-mem">{fmtBytes(p.mem)}</span>
            </li>
          {/each}
        </ol>
      </section>
    </div>
  {/if}
</main>

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
  .lamp.live {
    animation: pulse 2.4s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.25;
    }
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
  .p-proc { grid-column: span 12; }
  @media (max-width: 700px) {
    .p-gauge, .p-hist, .p-mem, .p-net { grid-column: span 12; }
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
    transition: height 0.6s ease;
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
    transition: width 0.6s ease;
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
    grid-template-columns: minmax(90px, 160px) 1fr 64px 76px;
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
    transition: width 0.6s ease;
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
    .panel,
    .lamp.live {
      animation: none;
    }
    .needle,
    .core-fill,
    .meter-fill,
    .pr-fill {
      transition: none;
    }
  }
</style>
