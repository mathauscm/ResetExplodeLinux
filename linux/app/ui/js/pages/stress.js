// Stress test: carga máxima de CPU, RAM e/ou GPU com corte automático por temperatura, tempo ou erro de memória.
// Quem vigia os limites é o backend (continua valendo com a tela fechada); aqui só se mostra e se comanda.

import { call, head, esc, num, every } from "../lib.js";

// Escolhas do formulário entre visitas (quando não há teste rodando).
const form = { cpu: true, ram: false, ram_fraction: 0.6, gpu: false, cpu_limit: 95, gpu_limit: 85, minutes: 10, gpu_res: "1920x1080", gpu_msaa: 4, gpu_intensity: "alta" };

const chips = (id, items, on) =>
  `<div class="chips" id="${id}">${items.map(([v, t]) => `<button class="chip${String(v) === String(on) ? " on" : ""}" data-v="${v}" type="button">${t}</button>`).join("")}</div>`;
const temp = (v) => (v == null ? "—" : `${num(v, v % 1 ? 1 : 0)} °C`);
const hms = (s) => [Math.floor(s / 3600), Math.floor((s % 3600) / 60), Math.floor(s % 60)].map((n) => String(n).padStart(2, "0")).join(":");
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const parse = (s) => Number(String(s).trim().replace(",", "."));

export function render(el, item) {
  el.innerHTML = `${head("RESET DESEMPENHO", item.label)}
    <p class="sub" style="margin:-10px 0 16px">Carga máxima com corte automático por temperatura</p>
    <div class="stress-layout">
      <div class="card opts stress-opts">
        <h2>Componentes</h2>
        <div class="comp-row">
          <label class="comp"><input type="checkbox" id="cpu"><span><b>CPU</b><small>todas as threads</small></span></label>
          <label class="comp"><input type="checkbox" id="ram"><span><b>RAM</b><small>grava e verifica</small></span></label>
          <label class="comp"><input type="checkbox" id="gpu"><span><b>GPU</b><small>reset-gpu</small></span></label>
        </div>
        <label class="field">Uso da RAM livre</label>
        ${chips("frac", [[0.4, "40%"], [0.6, "60%"], [0.8, "80%"]], form.ram_fraction)}
        <div class="lim-row">
          <div><label class="field">Limite CPU °C</label><input type="number" id="climit" min="40" max="110"></div>
          <div><label class="field">Limite GPU °C</label><input type="number" id="glimit" min="40" max="100"></div>
          <div><label class="field">Duração (min)</label><input type="number" id="minutes" min="0" title="0 = sem limite"></div>
        </div>
        <div id="gpuOpts" class="gpu-opts">
          <label class="field">GPU · resolução</label>
          ${chips("res", [["1280x720", "720p"], ["1920x1080", "1080p"], ["2560x1440", "1440p"], ["full", "Tela cheia"]], form.gpu_res)}
          <label class="field">Anti-aliasing</label>
          ${chips("msaa", [[0, "Off"], [4, "4x MSAA"], [8, "8x MSAA"]], form.gpu_msaa)}
          <label class="field">Intensidade</label>
          ${chips("int", [["normal", "Normal"], ["alta", "Alta"], ["extrema", "Extrema"]], form.gpu_intensity)}
        </div>
        <div class="row" style="margin-top:18px"><button class="btn" id="go" type="button">Iniciar</button><span class="faint small">duração 0 = sem limite</span></div>
        <div id="hint"></div>
      </div>

      <div class="stack">
        <div class="tiles">
          <div class="tile" id="statusTile"><span>Status</span><b id="tStatus">Parado</b><small id="tStatusSub">&nbsp;</small></div>
          <div class="tile"><span>Tempo</span><b id="tElapsed">00:00:00</b><small id="tLeft">&nbsp;</small></div>
          <div class="tile"><span>Temp. CPU</span><b id="tCpu">—</b><small id="tCpuMax">máx. —</small></div>
          <div class="tile"><span>Consumo CPU</span><b id="tCpuW">—</b><small id="tLoad">carga —</small></div>
          <div class="tile"><span>Temp. GPU</span><b id="tGpu">—</b><small id="tGpuMax">máx. —</small></div>
          <div class="tile"><span>Consumo GPU</span><b id="tGpuW">—</b><small id="tCap">—</small></div>
          <div class="tile"><span>FPS (GPU)</span><b id="tFps">—</b><small id="tFpsAvg">média —</small></div>
          <div class="tile"><span>RAM</span><b id="tRam">—</b><small id="tRamSub">passes / erros</small></div>
        </div>
        <div class="card">
          <div class="row chart-head"><h2>Temperaturas — últimos 5 min</h2><span class="spacer"></span>
            <span class="legend"><i class="lg-cpu"></i>CPU</span><span class="legend"><i class="lg-gpu"></i>GPU</span><span class="legend"><i class="lg-lim"></i>limites</span></div>
          <div class="chart-box"><canvas id="chart"></canvas></div>
        </div>
        <div class="card"><h2>Registro</h2><div class="stress-log" id="log"><div class="faint">Nenhum evento ainda.</div></div></div>
      </div>
    </div>`;

  const $ = (id) => el.querySelector("#" + id);
  let st = null; // último estado do backend
  let pending = null; // confirmação pendente (sensor faltando)

  // ---------------- formulário
  function fillForm(c) {
    $("cpu").checked = c.cpu;
    $("ram").checked = c.ram;
    $("gpu").checked = c.gpu;
    $("climit").value = c.cpu_limit;
    $("glimit").value = c.gpu_limit;
    $("minutes").value = c.minutes;
    for (const [id, v] of [["frac", c.ram_fraction], ["res", c.gpu_res], ["msaa", c.gpu_msaa], ["int", c.gpu_intensity]])
      $(id).querySelectorAll(".chip").forEach((b) => b.classList.toggle("on", b.dataset.v === String(v)));
  }
  function readForm() {
    const on = (id) => $(id).querySelector(".chip.on")?.dataset.v;
    Object.assign(form, {
      cpu: $("cpu").checked,
      ram: $("ram").checked,
      gpu: $("gpu").checked,
      cpu_limit: parse($("climit").value),
      gpu_limit: parse($("glimit").value),
      minutes: parse($("minutes").value),
      ram_fraction: Number(on("frac") ?? 0.6),
      gpu_res: on("res") ?? "1920x1080",
      gpu_msaa: Number(on("msaa") ?? 4),
      gpu_intensity: on("int") ?? "alta",
    });
    return form;
  }
  function syncEnabled() {
    const running = st?.state.running;
    el.querySelectorAll(".stress-opts input").forEach((i) => (i.disabled = running));
    el.querySelectorAll(".stress-opts .chip").forEach((b) => (b.disabled = running));
    if (!running) {
      $("frac").querySelectorAll(".chip").forEach((b) => (b.disabled = !$("ram").checked));
      $("glimit").disabled = !$("gpu").checked;
      el.querySelectorAll("#gpuOpts .chip").forEach((b) => (b.disabled = !$("gpu").checked));
    }
    el.querySelectorAll(".comp").forEach((c) => {
      const i = c.querySelector("input");
      c.classList.toggle("on", i.checked);
      c.classList.toggle("locked", i.disabled);
    });
    $("gpuOpts").classList.toggle("off", !$("gpu").checked);
    $("go").textContent = running ? "Parar" : "Iniciar";
    $("go").classList.toggle("outline", !!running);
  }
  el.querySelector(".stress-opts").addEventListener("click", (e) => {
    const b = e.target.closest(".chip");
    if (!b || b.disabled) return;
    b.parentElement.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c === b));
    readForm();
  });
  el.querySelector(".stress-opts").addEventListener("change", () => {
    readForm();
    syncEnabled();
    draw();
  });
  fillForm(form);
  syncEnabled();

  // ---------------- iniciar / parar
  const hint = (html) => ($("hint").innerHTML = html ? `<div class="test-banner stress-hint">${html}</div>` : "");

  function validate(c) {
    if (!c.cpu && !c.ram && !c.gpu) return "Ative CPU, RAM ou GPU.";
    if (!(c.cpu_limit >= 40 && c.cpu_limit <= 110)) return "Limite da CPU deve estar entre 40 e 110 °C.";
    if (c.gpu && !(c.gpu_limit >= 40 && c.gpu_limit <= 100)) return "Limite da GPU deve estar entre 40 e 100 °C.";
    if (!(c.minutes >= 0)) return "Duração inválida.";
    if (c.gpu && st && !st.gpu_tool)
      return "O programa reset-gpu não foi encontrado. Ele vem junto com o RESET EXPLODE: rode ./install-local.sh de novo ou, no desenvolvimento, <code>cargo build -p reset-gpu</code>.";
    if (st?.bench_running) return "Termine o benchmark antes de começar o stress test.";
    return null;
  }

  async function start(c) {
    hint("");
    $("go").disabled = true;
    try {
      await call("stress", "start", c);
    } catch (e) {
      hint(esc(String(e)));
    } finally {
      $("go").disabled = false;
    }
    await refresh();
  }

  $("go").addEventListener("click", async () => {
    if (st?.state.running) {
      $("go").disabled = true;
      await call("stress", "stop").catch((e) => hint(esc(String(e))));
      $("go").disabled = false;
      return refresh();
    }
    const c = { ...readForm() };
    const err = validate(c);
    if (err) return hint(err);
    // Sem sensor de temperatura o corte automático não funciona: pergunta antes, como no Windows.
    const s = st?.state.sample ?? {};
    const missing = [];
    if ((c.cpu || c.ram) && s.cpu_temp == null) missing.push("da CPU (no Linux vem do módulo <code>k10temp</code> ou <code>coretemp</code>)");
    if (c.gpu && s.gpu_temp == null) missing.push("da GPU (NVML/driver NVIDIA)");
    if (missing.length) {
      pending = c;
      hint(`Nenhum sensor de temperatura ${missing.join(" nem ")} foi encontrado, então o corte automático por temperatura
        <b>não funcionará</b>. Continuar mesmo assim?
        <div class="row" style="margin-top:10px"><button class="btn" id="yes" type="button">Continuar</button><button class="btn outline" id="no" type="button">Cancelar</button></div>`);
      $("yes").onclick = () => start(pending);
      $("no").onclick = () => hint("");
      return;
    }
    start(c);
  });

  // ---------------- estado ao vivo
  function show() {
    const s = st.state, x = s.sample;
    const tile = $("statusTile");
    tile.classList.toggle("alert", s.alert);
    tile.classList.toggle("running", s.running);
    $("tStatus").textContent = s.status;
    const parts = [s.config.cpu && "CPU", s.config.ram && "RAM", s.config.gpu && "GPU"].filter(Boolean).join(" · ");
    $("tStatusSub").innerHTML = s.running ? esc(parts) : "&nbsp;";
    $("tElapsed").textContent = hms(s.elapsed_s);
    const total = s.config.minutes * 60;
    $("tLeft").innerHTML = s.running ? (total > 0 ? `faltam ${hms(Math.max(0, total - s.elapsed_s))}` : "sem limite de tempo") : "&nbsp;";
    $("tCpu").textContent = temp(x.cpu_temp);
    $("tCpuMax").textContent = `máx. ${temp(s.cpu_max)}`;
    $("tCpuW").textContent = x.cpu_power == null ? "—" : `${num(x.cpu_power, 1)} W`;
    $("tCpuW").title = x.cpu_power == null ? "O consumo da CPU (RAPL) só pode ser lido como root no Linux." : "";
    $("tLoad").textContent = `carga ${x.cpu_load == null ? "—" : num(x.cpu_load) + "%"}`;
    $("tGpu").textContent = temp(x.gpu_temp);
    $("tGpuMax").textContent = `máx. ${temp(s.gpu_max)}`;
    $("tGpuW").textContent = x.gpu_power == null ? "—" : `${num(x.gpu_power, 1)} W`;
    $("tCap").textContent = x.gpu_perfcap ? `limitador: ${x.gpu_perfcap}` : "—";
    $("tFps").textContent = s.gpu_on && s.gpu_fps ? num(s.gpu_fps) : "—";
    $("tFpsAvg").textContent = `média ${s.gpu_fps_avg ? num(s.gpu_fps_avg) : "—"}`;
    $("tRam").textContent = s.ram_on ? `${num(s.ram_passes)} / ${num(s.ram_errors)}` : "—";
    $("tRam").classList.toggle("bad-text", s.ram_errors > 0);
    $("tRamSub").textContent = s.ram_on && s.ram_allocated ? `${num(s.ram_allocated / 1073741824, 1)} GB · passes/erros` : "passes / erros";
    const log = $("log");
    const atEnd = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
    log.innerHTML = s.log.length
      ? s.log.map((l) => `<div class="${/ABORTADO/.test(l) ? "log-bad" : ""}">${esc(l)}</div>`).join("")
      : `<div class="faint">Nenhum evento ainda.</div>`;
    if (atEnd) log.scrollTop = log.scrollHeight;
    if (s.running) fillForm(s.config);
    syncEnabled();
    draw();
  }

  async function refresh() {
    st = await call("stress", "status");
    show();
  }

  // ---------------- gráfico (canvas)
  function draw() {
    const cv = $("chart");
    if (!cv || !st) return;
    const s = st.state;
    const dpr = window.devicePixelRatio || 1;
    const w = cv.clientWidth, h = cv.clientHeight;
    if (w < 50 || h < 40) return;
    if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) {
      cv.width = Math.round(w * dpr);
      cv.height = Math.round(h * dpr);
    }
    const g = cv.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, h);
    const fg = css("--fg"), muted = css("--muted"), faint = css("--faint"), line = css("--line");
    const gpuOn = s.running ? s.config.gpu : $("gpu").checked;
    const climit = s.running ? s.config.cpu_limit : form.cpu_limit;
    const glimit = s.running ? s.config.gpu_limit : form.gpu_limit;
    const left = 34, top = 8, bottom = 8, plotW = w - left, plotH = h - top - bottom;
    const yMin = 20, yMax = Math.max(110, Math.max(climit || 0, gpuOn ? glimit || 0 : 0) + 10);
    const Y = (v) => top + plotH * (1 - (Math.min(yMax, Math.max(yMin, v)) - yMin) / (yMax - yMin));

    g.font = `10px ${css("--mono")}`;
    g.textBaseline = "middle";
    for (let v = yMin; v <= yMax; v += 20) {
      g.strokeStyle = line;
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(left, Math.round(Y(v)) + 0.5);
      g.lineTo(w, Math.round(Y(v)) + 0.5);
      g.stroke();
      g.fillStyle = faint;
      g.fillText(`${v}°`, 0, Y(v));
    }
    const limitLine = (v, color) => {
      if (!(v > 0)) return;
      g.save();
      g.setLineDash([4, 3]);
      g.strokeStyle = color;
      g.beginPath();
      g.moveTo(left, Math.round(Y(v)) + 0.5);
      g.lineTo(w, Math.round(Y(v)) + 0.5);
      g.stroke();
      g.restore();
    };
    limitLine(climit, muted);
    if (gpuOn) limitLine(glimit, faint);

    const step = plotW / (s.history_len - 1);
    const series = (data, color, fill) => {
      if (data.length < 2) return;
      const off = s.history_len - data.length; // alinha à direita
      const pts = data.map((v, i) => [left + (off + i) * step, Y(v)]);
      if (fill) {
        g.save();
        g.globalAlpha = 0.09;
        g.fillStyle = color;
        g.beginPath();
        g.moveTo(pts[0][0], top + plotH);
        pts.forEach(([x, y]) => g.lineTo(x, y));
        g.lineTo(pts[pts.length - 1][0], top + plotH);
        g.fill();
        g.restore();
      }
      g.strokeStyle = color;
      g.lineWidth = 1.6;
      g.lineJoin = "round";
      g.beginPath();
      pts.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y)));
      g.stroke();
      const [lx, ly] = pts[pts.length - 1];
      g.fillStyle = color;
      g.beginPath();
      g.arc(lx, ly, 2.5, 0, Math.PI * 2);
      g.fill();
    };
    series(s.history_gpu, muted, false);
    series(s.history_cpu, fg, true);
    if (s.history_cpu.length < 2 && s.history_gpu.length < 2) {
      g.fillStyle = faint;
      g.textAlign = "center";
      g.fillText("coletando leituras…", left + plotW / 2, top + plotH / 2);
      g.textAlign = "left";
    }
  }

  const onResize = () => draw();
  window.addEventListener("resize", onResize);
  const stop = every(1000, refresh);
  return () => {
    stop();
    window.removeEventListener("resize", onResize);
  };
}
