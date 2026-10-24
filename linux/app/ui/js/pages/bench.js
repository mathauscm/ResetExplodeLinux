// RESET BENCH: render de CPU (path tracing, imagem bloco a bloco), cena de jogo e ray tracing (reset-gpu),
// simulação de jogo, memória e disco. Um teste por vez; o backend manda o andamento por eventos.

import { call, listen, snapshot, head, esc, num, fmtTemp, fmtPct, fmtMhz, fmtW, kv } from "../lib.js";

const W = 960, H = 540, BUCKET = 32;

const TESTS = [
  ["render", "Render (CPU)", "Benchmark de CPU por renderização 3D (path tracing), no mesmo princípio do Cinebench"],
  ["gpu", "Cena de jogo (GPU)", "Cena de jogo em Vulkan para medir a placa de vídeo, no estilo 3DMark / Unigine"],
  ["rt", "Ray tracing (GPU)", "Ray tracing por hardware (Vulkan ray query): usa os núcleos de RT da placa"],
  ["sim", "Simulação de jogo (CPU)", "Quanto FPS a CPU aguenta numa simulação de jogo com milhares de unidades"],
  ["mem", "Memória e carregamento", "Latência e banda da RAM e velocidade de carregamento do disco"],
  ["disk", "Disco", "Velocidade de leitura e escrita do disco: sequencial e acessos aleatórios de 4 KB, no estilo CrystalDiskMark"],
];

// Fica entre visitas à página: a última imagem renderizada e as escolhas do usuário.
const keep = { image: null, test: "render", mode: "both", sustained: false, gpuRes: "1920x1080", gpuSecs: 30, rtRes: "2560x1440", rtPreset: "alta", rtMode: "std", fullscreen: false, simSecs: 20, disk: null, diskSize: 1024 };

const pad = (n) => String(n).padStart(2, "0");
const when = (s) => {
  const d = new Date(s * 1000);
  return `${pad(d.getDate())}/${pad(d.getMonth() + 1)} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
};
const mmss = (s) => `${pad(Math.floor(s / 60))}:${pad(Math.floor(s % 60))}`;
const chips = (id, items, on) =>
  `<div class="chips" id="${id}">${items.map(([v, t, tip]) => `<button class="chip${String(v) === String(on) ? " on" : ""}" data-v="${esc(v)}" type="button"${tip ? ` title="${esc(tip)}"` : ""}>${t}</button>`).join("")}</div>`;
const b64bytes = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** Linhas de histórico com barra relativa ao melhor resultado. */
function meters(rows) {
  if (!rows.length) return `<div class="sub">Nenhum resultado ainda.</div>`;
  return rows
    .map((r) => `<div class="meter"><span class="meter-when">${esc(r.label)}</span>
      <div class="meter-bar"><i style="width:${Math.max(2, Math.min(100, r.frac * 100))}%"></i></div>
      <span class="meter-val">${esc(r.value)}</span></div>`)
    .join("");
}

function recordRows(history, test) {
  const list = history.records.filter((r) => (typeof test === "function" ? test(r.test) : r.test === test));
  const best = {};
  for (const r of list) best[r.test] = Math.max(best[r.test] ?? 0, r.score);
  return list.map((r) => ({ label: when(r.date), value: r.summary, frac: best[r.test] > 0 ? r.score / best[r.test] : 0 }));
}

export async function render(el, item) {
  let info = { history: { runs: [], records: [] }, threads: navigator.hardwareConcurrency || 1 };
  let running = null; // teste rodando (nome) ou null
  let gpuInfo = null; // resposta do `reset-gpu info` (ou {error})
  const offs = [];
  let liveTimer = null;
  let alive = true;
  let blocked = false; // teste indisponível (sem reset-gpu, sem unidade...): botão fica desligado

  el.innerHTML = `${head("RESET DESEMPENHO", item.label)}
    <p class="sub bench-sub" id="sub"></p>
    <div class="chips bench-tests" id="tests">${TESTS.map(([id, name]) => `<button class="chip" data-t="${id}" type="button">${name}</button>`).join("")}</div>
    <div id="debugWarn"></div>
    <div id="panel"></div>`;
  const $ = (id) => el.querySelector("#" + id);

  // ------------------------------------------------------------ comum

  const setPaused = (on) => {
    // Consultas ao driver geram engasgos no frametime: a barra lateral pode olhar este sinal para pausar.
    if (on) document.documentElement.dataset.sensorsPaused = "1";
    else delete document.documentElement.dataset.sensorsPaused;
  };

  function selectTest(t) {
    if (keep.test === "render" && panelCanvas()) saveImage();
    keep.test = t;
    ctx = sctx = null;
    blocked = false;
    el.querySelectorAll("#tests .chip").forEach((c) => c.classList.toggle("on", c.dataset.t === t));
    $("sub").textContent = TESTS.find((x) => x[0] === t)[2];
    PANELS[t]();
    syncRunning();
  }

  function syncRunning() {
    const busy = !!running;
    el.querySelectorAll("#tests .chip").forEach((c) => (c.disabled = busy && c.dataset.t !== running));
    el.querySelectorAll("#panel .opts .chip, #panel .opts input").forEach((c) => (c.disabled = busy));
    const btn = $("go");
    if (btn) {
      const mine = running === keep.test;
      btn.textContent = mine ? (keep.test === "render" ? "Parar" : "Cancelar") : "Iniciar";
      btn.classList.toggle("outline", mine);
      btn.disabled = (busy && !mine) || (blocked && !mine);
    }
  }

  /** Chegou andamento de um teste: garante que a tela mostra que ele está rodando. */
  function mark(t) {
    if (running === t) return;
    running = t;
    syncRunning();
  }

  function bindChips(id, fn) {
    $(id)?.addEventListener("click", (e) => {
      const b = e.target.closest(".chip");
      if (!b || b.disabled) return;
      $(id).querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c === b));
      fn(b.dataset.v);
    });
  }

  async function go(method, args) {
    const hint = $("hint");
    if (running === keep.test) {
      await call("bench", "cancel");
      return false;
    }
    if (hint) hint.textContent = "";
    try {
      await call("bench", method, args);
      running = keep.test;
      syncRunning();
      return true;
    } catch (e) {
      if (hint) hint.textContent = String(e);
      return false;
    }
  }

  // ------------------------------------------------------------ render (CPU)

  let ctx = null, octx = null;
  const panelCanvas = () => el.querySelector("#img");
  function saveImage() {
    if (ctx) keep.image = ctx.getImageData(0, 0, W, H);
  }

  function renderPanel() {
    $("panel").innerHTML = `<div class="bench-layout">
      <div class="stack">
        <div class="card render-card">
          <div class="render-stage" id="stage">
            <canvas id="img" width="${W}" height="${H}"></canvas>
            <canvas id="ovl" width="${W}" height="${H}"></canvas>
            <div class="render-idle" id="idle"><b>RESET BENCH</b><span>Pressione INICIAR para renderizar a cena</span></div>
          </div>
          <div class="render-foot">
            <div class="thin-bar"><i id="prog"></i></div>
            <div class="row"><span id="status" class="sub">Pronto</span><span class="spacer"></span><span id="elapsed" class="mono-big">00:00</span></div>
          </div>
        </div>
        <div class="card"><h2>Histórico</h2><div id="hist"></div></div>
      </div>
      <div class="stack">
        <div class="card opts"><h2>Teste</h2>
          ${chips("mode", [["mt", "Multi-core"], ["st", "Single-core"], ["both", "Ambos"]], keep.mode)}
          <label class="field">Duração</label>
          ${chips("dur", [["0", "1 passada"], ["10", "Mínimo 10 min"]], keep.sustained ? "10" : "0")}
          <div class="row" style="margin-top:16px"><button class="btn" id="go" type="button">Iniciar</button><span class="sub" id="hint"></span></div>
        </div>
        <div class="card"><h2>Resultado</h2>
          <div class="score-pair">
            <div><span class="score-label">Multi-core</span><div class="score" id="mt">—<small>pts</small></div></div>
            <div><span class="score-label">Single-core</span><div class="score" id="st">—<small>pts</small></div></div>
          </div>
          <p class="sub" id="ratio"></p>
          <p class="faint small ref">Referência no app do Windows (.NET): Ryzen 7 7700X ≈ 4.500 multi / 530 single. Esta versão em Rust
            faz a mesma imagem e a mesma conta, mas roda ~25–35% mais rápido no mesmo PC: compare só com notas do Linux.</p>
        </div>
        <div class="card"><h2>Durante o teste</h2><div id="live"></div></div>
      </div>
    </div>`;
    ctx = $("img").getContext("2d");
    octx = $("ovl").getContext("2d");
    drawGrid();
    if (keep.image) {
      ctx.putImageData(keep.image, 0, 0);
      $("idle").hidden = true;
    }
    bindChips("mode", (v) => (keep.mode = v));
    bindChips("dur", (v) => (keep.sustained = v === "10"));
    $("go").addEventListener("click", async () => {
      const was = running;
      if (await go("render_start", { mode: keep.mode, sustained: keep.sustained }) && !was) {
        ctx.clearRect(0, 0, W, H);
        keep.image = null;
        $("idle").hidden = true;
        $("mt").innerHTML = $("st").innerHTML = "—<small>pts</small>";
        $("ratio").textContent = "";
        $("status").textContent = "Começando…";
        lastPass = 0;
      }
    });
    drawRenderHistory();
    liveCard();
  }

  /** Grade dos blocos ao fundo (o que ainda não foi renderizado). */
  function drawGrid() {
    const g = $("stage");
    if (g) g.style.setProperty("--grid", `${(BUCKET / W) * 100}%`);
  }

  let lastPass = 0;
  function onRender(p) {
    if (keep.test !== "render" || !ctx) return;
    mark("render");
    $("idle").hidden = true;
    const key = `${p.phase}#${p.pass}`;
    if (key !== lastPass) {
      // fase ou passada nova: escurece a imagem anterior em vez de apagar
      if (lastPass) {
        ctx.fillStyle = "rgba(0,0,0,.6)";
        ctx.fillRect(0, 0, W, H);
      }
      lastPass = key;
    }
    for (const t of p.tiles) {
      const g = b64bytes(t.data);
      const img = ctx.createImageData(t.w, t.h);
      for (let i = 0; i < g.length; i++) {
        const o = i * 4;
        img.data[o] = img.data[o + 1] = img.data[o + 2] = g[i];
        img.data[o + 3] = 255;
      }
      ctx.putImageData(img, t.x, t.y);
    }
    // blocos em andamento: cantos em destaque, como o Cinebench
    octx.clearRect(0, 0, W, H);
    for (const [x, y, w, h] of p.active) {
      octx.fillStyle = "rgba(255,255,255,.07)";
      octx.fillRect(x, y, w, h);
      octx.strokeStyle = "#fff";
      octx.lineWidth = 2;
      octx.shadowColor = "rgba(0,0,0,.8)";
      octx.shadowBlur = 3;
      const k = 8, a = x + 1, b = y + 1, c = x + w - 1, d = y + h - 1;
      octx.beginPath();
      octx.moveTo(a, b + k); octx.lineTo(a, b); octx.lineTo(a + k, b);
      octx.moveTo(c - k, b); octx.lineTo(c, b); octx.lineTo(c, b + k);
      octx.moveTo(c, d - k); octx.lineTo(c, d); octx.lineTo(c - k, d);
      octx.moveTo(a + k, d); octx.lineTo(a, d); octx.lineTo(a, d - k);
      octx.stroke();
      octx.shadowBlur = 0;
    }
    const frac = Math.min(1, p.frac);
    $("prog").style.width = `${frac * 100}%`;
    const eta = frac >= 0.03 ? ` · faltam ~${mmss((p.phase_elapsed * (1 - frac)) / frac)}` : "";
    const pass = p.sustained ? ` · passada ${p.pass} (mínimo 10 min)` : "";
    $("status").textContent = `${p.phase}${pass} · ${num(frac * 100)}%${p.sustained ? "" : eta}`;
    $("elapsed").textContent = mmss(p.elapsed);
    if (p.multi != null) $("mt").innerHTML = `${num(p.multi)}<small>pts</small>`;
    if (p.single != null) $("st").innerHTML = `${num(p.single)}<small>pts</small>`;
  }

  function renderDone(d) {
    if (!ctx) return;
    octx.clearRect(0, 0, W, H);
    $("prog").style.width = "0%";
    saveImage();
    if (!d.ok) {
      $("status").textContent = d.cancelled ? "Cancelado" : `Falhou: ${d.error}`;
      return;
    }
    const r = d.result;
    $("status").textContent = `Concluído em ${mmss(r.seconds)}`;
    $("mt").innerHTML = `${r.multi != null ? num(r.multi) : "—"}<small>pts</small>`;
    $("st").innerHTML = `${r.single != null ? num(r.single) : "—"}<small>pts</small>`;
    if (r.multi && r.single)
      $("ratio").textContent = `Multi ÷ single: ${num(r.multi / r.single, 1)}x com ${r.threads} threads (${Math.round(r.threads / 2)} núcleos com SMT costumam render 8–12x).`;
  }

  function drawRenderHistory() {
    const runs = info.history.runs;
    const bestMt = Math.max(1, ...runs.map((r) => r.multi ?? 0));
    const bestSt = Math.max(1, ...runs.map((r) => r.single ?? 0));
    const h = $("hist");
    if (h)
      h.innerHTML = meters(
        runs.map((r) => ({
          label: `${when(r.date)}${r.sustained ? " · 10 min" : ""}`,
          value: `${r.multi != null ? num(r.multi) : "—"} / ${r.single != null ? num(r.single) : "—"}${r.cpu ? ` · ${r.cpu}` : ""}`,
          frac: r.multi != null ? r.multi / bestMt : r.single != null ? r.single / bestSt : 0,
        }))
      ) + (runs.length ? `<div class="faint small" style="margin-top:8px">multi / single, em pontos</div>` : "");
  }

  // Temperatura, clock, consumo e carga da CPU enquanto renderiza.
  function liveCard() {
    clearInterval(liveTimer);
    const tick = async () => {
      const box = $("live");
      if (!box || !alive) return;
      if (document.documentElement.dataset.sensorsPaused) {
        box.innerHTML = `<div class="sub">Sensores pausados durante o teste.</div>`;
        return;
      }
      const { summary: s, rows } = await snapshot();
      const power = rows.find((r) => r.hardware === "CPU" && r.kind === "power");
      box.innerHTML = kv([
        ["CPU", info.cpu_short ?? "—"],
        ["Threads", String(info.threads)],
        ["Temperatura", fmtTemp(s.cpu_temp_c)],
        ["Clock médio", fmtMhz(s.cpu_mhz_avg)],
        ["Consumo", power ? fmtW(power.value) : "— (só como root)"],
        ["Carga", fmtPct(s.cpu_load_pct)],
      ]);
    };
    tick().catch(() => {});
    liveTimer = setInterval(() => tick().catch(() => {}), 1000);
  }

  // ------------------------------------------------------------ GPU (reset-gpu)

  async function loadGpuInfo() {
    if (gpuInfo) return gpuInfo;
    if (!info.gpu_tool) return (gpuInfo = { missing: true });
    try {
      gpuInfo = (await call("bench", "gpu_info")) ?? {};
    } catch (e) {
      return { error: String(e) }; // não guarda: tenta de novo na próxima vez
    }
    return gpuInfo;
  }

  function gpuPanel(rt) {
    const res = [["1920x1080", "1080p"], ["2560x1440", "1440p"], ["3840x2160", "4K"]];
    const opts = rt
      ? `<label class="field">Resolução</label>${chips("res", res, keep.rtRes)}
         <label class="field">Preset</label>${chips("preset", [["normal", "Normal"], ["alta", "Alta"], ["extrema", "Extrema"]], keep.rtPreset)}
         <label class="field">Modo</label>${chips("rmode", [["std", "Padrão", "~90 s medindo. Use para comparar."], ["quick", "Rápido", "~30 s. Só para uma ideia rápida."]], keep.rtMode)}`
      : `<label class="field">Resolução</label>${chips("res", res, keep.gpuRes)}
         <label class="field">Duração</label>${chips("secs", [["30", "30 s"], ["60", "60 s"]], keep.gpuSecs)}`;
    $("panel").innerHTML = `<div class="bench-layout">
      <div class="stack">
        <div class="card gpu-stage">
          <h2>${rt ? "Ray tracing por hardware" : "Cena de jogo"}</h2>
          <div class="gpu-now">
            <div><span class="score-label">FPS agora</span><div class="score huge" id="fps">—</div></div>
            <div><span class="score-label">GPU</span><div class="score" id="gtemp">—</div><div class="sub" id="gpow"></div></div>
          </div>
          <div class="thin-bar"><i id="prog"></i></div>
          <div class="row" style="margin-top:8px"><span id="status" class="sub">Pronto</span><span class="spacer"></span><span id="elapsed" class="mono-big">00:00</span></div>
          <div id="gpuMsg"></div>
          <p class="faint small" style="margin-top:12px">O teste abre uma janela própria (Vulkan). <b>ESC</b> ou fechar a janela cancela.
            Durante o teste os sensores ficam pausados: as consultas ao driver geram engasgos no 1% low.</p>
        </div>
        ${rt ? `<div class="card"><h2>Como comparar com confiança</h2><p class="sub">Compare só resultados com a mesma resolução e o mesmo preset, com o PC
          na tomada, sem outros programas pesados abertos e com o mesmo driver. A cena é sempre a mesma (sementes fixas), em qualquer PC.
          As notas do Linux (Vulkan) não são comparáveis com as do app do Windows (Direct3D 12).</p></div>` : ""}
        <div class="card"><h2>Histórico</h2><div id="hist"></div></div>
      </div>
      <div class="stack">
        <div class="card opts">${opts}
          <label class="row sub" style="margin-top:12px"><input type="checkbox" id="full" ${keep.fullscreen ? "checked" : ""}> tela cheia</label>
          <div class="row" style="margin-top:16px"><button class="btn" id="go" type="button">Iniciar</button><span class="sub" id="hint"></span></div>
        </div>
        <div class="card"><h2>Resultado</h2><div id="result"><div class="sub">—</div></div></div>
        <div class="card"><h2>Placa</h2><div id="adapter"><div class="sub">Consultando o reset-gpu…</div></div></div>
      </div>
    </div>`;
    bindChips("res", (v) => (rt ? (keep.rtRes = v) : (keep.gpuRes = v)));
    bindChips("secs", (v) => (keep.gpuSecs = Number(v)));
    bindChips("preset", (v) => (keep.rtPreset = v));
    bindChips("rmode", (v) => (keep.rtMode = v));
    $("full").addEventListener("change", (e) => (keep.fullscreen = e.target.checked));
    const test = rt ? "rt" : "gpu";
    $("go").addEventListener("click", async () => {
      const [w, h] = (rt ? keep.rtRes : keep.gpuRes).split("x").map(Number);
      const seconds = rt ? (keep.rtMode === "quick" ? 30 : 90) : keep.gpuSecs;
      gpuSeconds = seconds;
      const was = running;
      if (await go("gpu_start", { test, width: w, height: h, seconds, preset: keep.rtPreset, fullscreen: keep.fullscreen }) && !was) {
        $("status").textContent = "Abrindo a janela do teste…";
        $("result").innerHTML = `<div class="sub">—</div>`;
        setPaused(true);
      }
    });
    $("hist").innerHTML = meters(recordRows(info.history, rt ? (t) => t.startsWith("rt:") : "gpu"));
    loadGpuInfo().then((g) => {
      if (keep.test !== test || !$("adapter")) return;
      if (g.missing || g.error) {
        $("adapter").innerHTML = `<div class="sub">—</div>`;
        $("gpuMsg").innerHTML = `<div class="test-banner">${esc(g.error ?? "O programa reset-gpu não foi encontrado. Ele faz os testes de GPU numa janela própria (Vulkan) e vem junto com o RESET EXPLODE: rode ./install-local.sh de novo ou, no desenvolvimento, cargo build -p reset-gpu.")}</div>`;
        blocked = true;
        syncRunning();
        return;
      }
      $("adapter").innerHTML = kv([
        ["Placa", g.adapter],
        ["API", g.backend],
        ["Driver", g.driver],
        ["Ray tracing (ray query)", g.ray_query == null ? null : g.ray_query ? "sim" : "não"],
      ]);
      if (rt && g.ray_query === false) {
        $("gpuMsg").innerHTML = `<div class="test-banner">Esta placa (ou o driver) não tem ray tracing por hardware. É preciso uma RTX 20 ou mais nova, Radeon RX 6000+ ou Intel Arc.</div>`;
        blocked = true;
        syncRunning();
      }
    });
  }

  let gpuSeconds = 30;
  function onGpu(p) {
    if (keep.test !== p.test || !$("fps")) return;
    mark(p.test);
    const ev = p.event;
    if (ev.type === "progress") {
      $("fps").textContent = ev.fps != null ? num(ev.fps) : "—";
      $("gtemp").textContent = fmtTemp(ev.temp_c);
      $("gpow").textContent = ev.power_w != null ? fmtW(ev.power_w) : "";
      const frac = Math.min(1, (ev.elapsed ?? 0) / gpuSeconds);
      $("prog").style.width = `${frac * 100}%`;
      $("elapsed").textContent = mmss(ev.elapsed ?? 0);
      $("status").textContent = `Rodando · ${num(frac * 100)}%`;
    } else if (ev.type === "info" && ev.adapter) {
      $("status").textContent = `Rodando em ${ev.adapter}`;
    }
  }

  function gpuDone(d) {
    setPaused(false);
    if (!$("fps")) return;
    $("prog").style.width = "0%";
    $("fps").textContent = "—";
    if (!d.ok) {
      $("status").textContent = d.cancelled ? "Cancelado" : `Falhou: ${d.error}`;
      return;
    }
    const r = d.result ?? {};
    $("status").textContent = "Concluído";
    $("result").innerHTML = `<div class="score">${num(r.score)}<small>pts</small></div>` + kv([
      ["FPS médio", r.avg_fps != null ? num(r.avg_fps, 1) : null],
      ["1% low", r.low1_fps != null ? num(r.low1_fps, 1) : null],
      ["Frames", r.frames != null ? num(r.frames) : null],
      ["Duração", r.seconds != null ? `${num(r.seconds, 1)} s` : null],
      ["Parou por", r.stopped],
    ]);
  }

  // ------------------------------------------------------------ simulação de jogo (CPU)

  let sctx = null;
  function simPanel() {
    $("panel").innerHTML = `<div class="bench-layout">
      <div class="stack">
        <div class="card render-card">
          <div class="sim-stage">
            <canvas id="sim" width="440" height="440"></canvas>
            <div class="render-idle" id="idle"><b>30 mil unidades em batalha</b><span>Pressione INICIAR</span></div>
            <div class="sim-fps" id="simFps"></div>
          </div>
          <div class="render-foot">
            <div class="thin-bar"><i id="prog"></i></div>
            <div class="row"><span id="status" class="sub">Pronto</span></div>
          </div>
        </div>
        <div class="card"><h2>Histórico</h2><div id="hist"></div></div>
      </div>
      <div class="stack">
        <div class="card opts"><h2>Duração</h2>${chips("secs", [["20", "20 s"], ["60", "60 s"]], keep.simSecs)}
          <div class="row" style="margin-top:16px"><button class="btn" id="go" type="button">Iniciar</button><span class="sub" id="hint"></span></div>
          <p class="faint small" style="margin-top:12px">Por frame: a thread principal monta a grade e roda a lógica (em série, como a maioria das
            engines); as outras fazem física, colisão e IA em paralelo. Antes de medir, 2 s de aquecimento.</p>
        </div>
        <div class="card"><h2>Resultado</h2><div id="result"><div class="sub">—</div></div>
          <p class="faint small ref">Referência no app do Windows (.NET): Ryzen 7 7700X ≈ 290 FPS. Esta versão em Rust roda ~35% mais
            rápido no mesmo PC: compare só com notas do Linux.</p></div>
      </div>
    </div>`;
    sctx = $("sim").getContext("2d");
    bindChips("secs", (v) => (keep.simSecs = Number(v)));
    $("go").addEventListener("click", async () => {
      const was = running;
      if (await go("sim_start", { seconds: keep.simSecs }) && !was) {
        $("idle").hidden = true;
        $("status").textContent = "Aquecendo (2 s)…";
        $("result").innerHTML = `<div class="sub">—</div>`;
      }
    });
    $("hist").innerHTML = meters(recordRows(info.history, "sim"));
  }

  function onSim(p) {
    if (keep.test !== "sim" || !sctx) return;
    mark("sim");
    $("idle").hidden = true;
    const n = p.size, cell = 440 / n, v = b64bytes(p.view);
    sctx.fillStyle = css("--bg");
    sctx.fillRect(0, 0, 440, 440);
    const colors = [null, css("--fg"), css("--muted"), css("--fg")]; // time A claro, time B cinza
    for (let k = 1; k <= 3; k++) {
      sctx.fillStyle = colors[k];
      sctx.beginPath();
      for (let i = 0; i < n * n; i++) if (((v[i >> 2] >> ((i & 3) * 2)) & 3) === k) sctx.rect((i % n) * cell, Math.floor(i / n) * cell, cell, cell);
      sctx.fill();
    }
    $("prog").style.width = `${p.progress * 100}%`;
    $("simFps").textContent = p.fps > 0 ? `${num(p.fps)} FPS` : "";
    if (!p.warming) $("status").textContent = `Simulando · ${num(p.progress * 100)}%`;
  }

  function simDone(d) {
    if (!sctx) return;
    $("prog").style.width = "0%";
    $("simFps").textContent = "";
    if (!d.ok) {
      $("status").textContent = d.cancelled ? "Cancelado" : `Falhou: ${d.error}`;
      return;
    }
    const r = d.result;
    $("status").textContent = `Concluído · ${num(r.frames)} frames em ${num(r.seconds)} s`;
    $("result").innerHTML = `<div class="score">${num(r.avg_fps)}<small>FPS</small></div>` + kv([
      ["1% low", num(r.low1_fps)],
      ["Thread principal", `${num(r.main_thread_ms, 2)} ms/frame`],
      ["Física + IA (paralelo)", `${num(r.parallel_ms, 2)} ms/frame`],
      ["Unidades", "30.000"],
    ]);
  }

  // ------------------------------------------------------------ memória e disco

  const resultRows = (rows) =>
    rows.length
      ? rows.map((r) => `<div class="res-row"><div><b>${esc(r.label)}</b><div class="sub">${esc(r.hint)}</div></div><span class="res-val">${esc(r.value)}</span></div>`).join("")
      : `<div class="sub">—</div>`;

  function memPanel() {
    $("panel").innerHTML = `<div class="bench-layout">
      <div class="stack">
        <div class="card"><h2>Resultados</h2><div id="rows">${resultRows([])}</div>
          <div class="thin-bar" style="margin-top:14px"><i id="prog"></i></div>
          <div class="row" style="margin-top:8px"><span id="status" class="sub">Pronto</span></div></div>
        <div class="card"><h2>Histórico</h2><div id="hist"></div></div>
      </div>
      <div class="stack">
        <div class="card opts"><h2>Teste</h2>
          <p class="sub">Latência e banda da RAM (usa ~2,3 GB por alguns segundos) e "carregamento de mapa": um pacote de 512 MB e mil
            arquivos de 64 KB gravados em <code>~/.cache</code>, lidos sem o cache do sistema e apagados no final.</p>
          <div class="row" style="margin-top:16px"><button class="btn" id="go" type="button">Iniciar</button><span class="sub" id="hint"></span></div>
        </div>
      </div>
    </div>`;
    $("go").addEventListener("click", async () => {
      const was = running;
      if (await go("mem_start") && !was) {
        $("rows").innerHTML = resultRows([]);
        $("status").textContent = "Começando…";
      }
    });
    $("hist").innerHTML = meters(recordRows(info.history, "mem"));
  }

  let drives = [];
  async function diskPanel() {
    $("panel").innerHTML = `<div class="bench-layout">
      <div class="stack">
        <div class="card"><h2>Resultados</h2><div id="rows">${resultRows([])}</div>
          <div class="thin-bar" style="margin-top:14px"><i id="prog"></i></div>
          <div class="row" style="margin-top:8px"><span id="status" class="sub">Pronto</span></div></div>
        <div class="card"><h2>Histórico</h2><div id="hist"></div></div>
      </div>
      <div class="stack">
        <div class="card opts"><h2>Unidade</h2><div id="drives"><div class="sub">Procurando unidades…</div></div>
          <label class="field">Tamanho do teste</label>${chips("size", [["512", "512 MB"], ["1024", "1 GB"], ["4096", "4 GB"]], keep.diskSize)}
          <div class="row" style="margin-top:16px"><button class="btn" id="go" type="button">Iniciar</button><span class="sub" id="hint"></span></div>
          <p class="faint small" style="margin-top:12px">Sem o cache do sistema (O_DIRECT). O arquivo de teste é criado numa pasta própria da
            unidade e apagado no final. Os sensores ficam pausados durante o teste.</p>
        </div>
      </div>
    </div>`;
    bindChips("size", (v) => (keep.diskSize = Number(v)));
    $("go").addEventListener("click", async () => {
      if (!keep.disk && !running) return ($("hint").textContent = "Escolha uma unidade.");
      const was = running;
      if (await go("disk_start", { mount: keep.disk, size_mb: keep.diskSize }) && !was) {
        $("rows").innerHTML = resultRows([]);
        $("status").textContent = "Preparando…";
        setPaused(true);
      }
    });
    $("hist").innerHTML = meters(recordRows(info.history, "disk"));
    try {
      drives = await call("bench", "disk_drives");
    } catch {
      drives = [];
    }
    if (keep.test !== "disk" || !$("drives")) return;
    if (!drives.length) {
      $("drives").innerHTML = `<div class="sub">Nenhuma unidade gravável com espaço livre suficiente (1,5 GB).</div>`;
      blocked = true;
      syncRunning();
      return;
    }
    if (!drives.some((d) => d.mount === keep.disk)) keep.disk = drives[0].mount;
    $("drives").innerHTML = chips("drv", drives.map((d) => [d.mount, esc(d.text)]), keep.disk);
    bindChips("drv", (v) => (keep.disk = v));
    syncRunning();
  }

  function onProgress(p) {
    if (keep.test !== p.test || !$("prog")) return;
    mark(p.test);
    $("prog").style.width = `${Math.min(1, p.frac) * 100}%`;
    $("status").textContent = p.phase;
    if (p.rows) $("rows").innerHTML = resultRows(p.rows);
  }

  function rowsDone(d) {
    if (d.test === "disk") setPaused(false);
    if (!$("rows")) return;
    $("prog").style.width = "0%";
    if (!d.ok) {
      $("status").textContent = d.cancelled ? "Cancelado (arquivos de teste apagados)" : `Falhou: ${d.error}`;
      return;
    }
    $("rows").innerHTML = resultRows(d.result.rows);
    $("status").textContent = d.test === "disk" ? `Concluído · arquivo de teste (${keep.diskSize} MB) apagado` : "Concluído · arquivos de teste apagados";
  }

  // ------------------------------------------------------------ montagem

  const PANELS = { render: renderPanel, gpu: () => gpuPanel(false), rt: () => gpuPanel(true), sim: simPanel, mem: memPanel, disk: diskPanel };

  offs.push(
    listen("bench-render", onRender),
    listen("bench-sim", onSim),
    listen("bench-progress", onProgress),
    listen("bench-gpu", onGpu),
    listen("bench-done", (d) => {
      running = null;
      info.history = d.history ?? info.history;
      if (d.test === keep.test) {
        ({ render: renderDone, gpu: gpuDone, rt: gpuDone, sim: simDone, mem: rowsDone, disk: rowsDone })[d.test](d);
        if (d.test === "render") drawRenderHistory();
        else if ($("hist")) $("hist").innerHTML = meters(recordRows(info.history, d.test === "rt" ? (t) => t.startsWith("rt:") : d.test));
      } else if (d.test === "gpu" || d.test === "rt" || d.test === "disk") setPaused(false);
      syncRunning();
    })
  );

  el.querySelector("#tests").addEventListener("click", (e) => {
    const c = e.target.closest(".chip");
    if (c && !c.disabled && c.dataset.t !== keep.test) selectTest(c.dataset.t);
  });

  try {
    info = await call("bench", "info");
  } catch (e) {
    console.error(e);
  }
  if (!alive) return;
  running = info.running ?? null;
  if (info.debug)
    $("debugWarn").innerHTML = `<div class="test-banner">Compilação de desenvolvimento (debug): as notas de CPU ficam muito abaixo do real.
      Rode com <code>cargo run --release -p reset-explode</code> ou use o app instalado.</div>`;
  if (info.stress_running) $("debugWarn").insertAdjacentHTML("beforeend", `<div class="test-banner">O stress test está rodando: pare-o antes de medir.</div>`);
  selectTest(running ?? keep.test);

  return () => {
    alive = false;
    clearInterval(liveTimer);
    if (keep.test === "render") saveImage();
    offs.forEach((p) => p.then((off) => off()));
    // teste de GPU/disco seguindo sem a página: a barra lateral volta a ler os sensores
    setPaused(false);
  };
}
