// Overclock: ajustes da GPU NVIDIA pelo NVML (limite de energia, offsets de clock e teto de clock), verificador
// de estabilidade, erros de hardware do log do kernel, o que dá para ver da CPU e o guia de overclock na BIOS.
// Aplicar pede a senha (pkexec) e roda o próprio app no modo --nvml-apply; tudo volta ao padrão ao reiniciar.

import { call, head, kv, esc, num, fmtTemp, fmtMhz, fmtW, bar, every } from "../lib.js";
import { runJob } from "../pkgjob.js";

const TARGETS = [["cpu", "CPU + RAM"], ["gpu", "GPU"], ["both", "Ambos"], ["watch", "Só observar"]];
const DURATIONS = [1, 5, 15, 30];

// O teto aplicado não pode ser lido de volta do driver: fica lembrado enquanto o app está aberto
// (e some ao reiniciar o PC, junto com o próprio teto).
let appliedCap;

const store = (k, v) => {
  try {
    if (v === undefined) return JSON.parse(localStorage.getItem("re.oc." + k) ?? "null");
    localStorage.setItem("re.oc." + k, JSON.stringify(v));
  } catch {
    return null;
  }
};
const signed = (v) => (v > 0 ? `+${num(v)}` : num(v));

export async function render(el) {
  el.innerHTML = head("RESET DESEMPENHO", "Overclock") + `<div class="empty">Consultando o driver…</div>`;
  const info = await call("oc", "info");
  let g = info.gpus[0] ?? null;
  const L = g?.limits;

  const startCap = appliedCap ?? null;
  const caps = g?.max_core_mhz
    ? [[null, "Sem teto"], ...[["Leve", 300], ["Equilibrado", 500], ["Econômico", 800]].map(([n, d]) => [g.max_core_mhz - d, `${n} · ${num(g.max_core_mhz - d)} MHz`])]
        .filter(([c]) => c == null || c >= L.cap_min)
    : [[null, "Sem teto"]];
  let want = g && {
    power_w: Math.round(g.power_limit_w ?? L.power_default_w),
    core_offset: g.core_offset?.current ?? 0,
    mem_offset: g.mem_offset?.current ?? 0,
    clock_cap: startCap,
  };
  let now = want && { ...want };
  let busy = false;

  const gpuTitle = g ? esc(g.name.replace(/^NVIDIA\s+/, "")) : "sem GPU NVIDIA";
  el.innerHTML = head("RESET DESEMPENHO", "Overclock", `<span class="tag">${gpuTitle}</span>`) + `
    <div class="oc-risk"><b>Atenção: overclock tem risco.</b> Valores altos demais travam o PC, podem corromper arquivos abertos e,
      no limite, danificar o hardware ou a garantia. O app só aceita uma faixa conservadora, pede a senha a cada mudança e tudo volta
      ao padrão ao reiniciar o PC. Mude um pouco por vez e rode o verificador depois de cada ajuste.</div>
    <div class="oc-top">
      <div class="card"><div class="row"><h2 style="margin:0">GPU agora</h2><span class="spacer"></span><span class="tag" id="pstate">—</span></div>
        <div id="gpuLive">${g ? "" : `<p class="sub" style="margin-top:10px">Nenhuma placa NVIDIA encontrada pelo NVML. Os ajustes de GPU são só para NVIDIA com o driver oficial; AMD fica para depois (sysfs do amdgpu).</p>`}</div></div>
      <div class="card"><h2>Ajustes da GPU</h2><div id="tune"></div></div>
    </div>
    <div class="card job" id="job" hidden style="margin-top:12px"></div>
    <div class="oc-mid">
      <div class="card"><h2>Verificador de estabilidade</h2><div id="check"></div></div>
      <div class="stack">
        <div class="card"><h2>Erros de hardware recentes</h2><div id="errors"><div class="sub">Lendo o log do kernel…</div></div></div>
        <div class="card"><h2>Processador</h2><div id="cpu"></div></div>
      </div>
    </div>
    <div class="card" style="margin-top:12px" id="guide"></div>`;

  const $ = (id) => el.querySelector("#" + id);

  // ---------------- GPU ao vivo ----------------
  function paintLive() {
    if (!g) return;
    const l = g.live;
    const tile = (label, val, sub) => `<div class="oc-tile"><span>${label}</span><b>${val}</b><small>${sub}</small></div>`;
    const cap = l.perfcap.filter((x) => x !== "Ocioso");
    $("pstate").textContent = l.pstate ? `${l.pstate}${l.perfcap.includes("Ocioso") ? " · ocioso" : ""}` : "—";
    $("gpuLive").innerHTML = `
      <div class="oc-tiles">
        ${tile("Núcleo", `${num(l.core_mhz)}<u>MHz</u>`, `máx. ${fmtMhz(g.max_core_mhz)}`)}
        ${tile("Memória", `${num(l.mem_mhz)}<u>MHz</u>`, `máx. ${fmtMhz(g.max_mem_mhz)}`)}
        ${tile("Temperatura", `${num(l.temp_c)}<u>°C</u>`, g.slowdown_c ? `reduz clock a ${g.slowdown_c} °C` : "")}
        ${tile("Energia", `${num(g.power_limit_w)}<u>W</u>`, l.power_w == null ? "limite · consumo não informado" : `limite · usando ${fmtW(l.power_w)}`)}
      </div>
      <div style="margin:12px 0 4px">${bar(l.load_pct)}</div>
      <div class="row sub"><span>Uso ${num(l.load_pct)}%</span><span class="spacer"></span><span>Limitado por: <b class="fg">${cap.length ? esc(cap.join(", ")) : "nada"}</b></span></div>
      <div style="margin-top:12px">${kv([
        ["Limite de energia", L.power_max_w > 0 ? `mín. ${fmtW(L.power_min_w)} · padrão ${fmtW(L.power_default_w)} · máx. ${fmtW(L.power_max_w)}` : null],
        ["Offset do núcleo", g.core_offset ? `${signed(g.core_offset.current)} MHz` : "não ajustável neste driver"],
        ["Offset da memória", g.mem_offset ? `${signed(g.mem_offset.current)} MHz` : "não ajustável neste driver"],
        ["Teto de clock", appliedCap === undefined ? "nenhum aplicado por este app" : appliedCap ? `${num(appliedCap)} MHz` : "nenhum"],
        ["Ventoinha", l.fan_pct == null ? null : `${num(l.fan_pct)}%`],
        ["Desliga a", g.shutdown_c ? `${g.shutdown_c} °C (proteção do driver)` : null],
      ])}</div>`;
  }

  // ---------------- ajustes ----------------
  function slider(id, label, value, min, max, step, unit, ticks) {
    return `<div class="oc-slider"><div class="row"><span class="oc-sl-label">${label}</span><span class="spacer"></span>
        <b class="oc-sl-val" id="${id}Val">${value}</b></div>
      <input type="range" id="${id}" min="${min}" max="${max}" step="${step}">
      <div class="oc-ticks">${ticks.map((t) => `<span>${t}</span>`).join("")}</div></div>`;
  }

  function changes() {
    const out = [];
    if (want.power_w !== now.power_w) out.push(`energia ${now.power_w} → ${want.power_w} W`);
    if (want.core_offset !== now.core_offset) out.push(`núcleo ${signed(now.core_offset)} → ${signed(want.core_offset)} MHz`);
    if (want.mem_offset !== now.mem_offset) out.push(`memória ${signed(now.mem_offset)} → ${signed(want.mem_offset)} MHz`);
    if (want.clock_cap !== now.clock_cap) out.push(`teto ${now.clock_cap ? now.clock_cap + " MHz" : "nenhum"} → ${want.clock_cap ? want.clock_cap + " MHz" : "nenhum"}`);
    return out;
  }

  function paintTune() {
    if (!g) {
      $("tune").innerHTML = `<p class="sub">Sem placa NVIDIA não há o que ajustar aqui.</p>`;
      return;
    }
    const core = L.core, mem = L.mem;
    $("tune").innerHTML = `
      ${slider("power", "Limite de energia", "", L.power_min_w, L.power_max_w, 1, "W", [`${num(L.power_min_w)} W`, `padrão ${num(L.power_default_w)} W`, `${num(L.power_max_w)} W`])}
      ${core ? slider("core", "Offset do núcleo", "", core[0], core[1], 15, "MHz", [`${signed(core[0])}`, "0", `${signed(core[1])} MHz`]) : `<div class="oc-slider"><span class="oc-sl-label">Offset do núcleo</span><div class="sub">O driver não permite nesta placa.</div></div>`}
      ${mem ? slider("mem", "Offset da memória", "", mem[0], mem[1], 50, "MHz", [`${signed(mem[0])}`, "0", `${signed(mem[1])} MHz`]) : `<div class="oc-slider"><span class="oc-sl-label">Offset da memória</span><div class="sub">O driver não permite nesta placa.</div></div>`}
      <div class="oc-slider"><span class="oc-sl-label">Teto de clock do núcleo</span>
        <div class="chips" id="caps" style="margin-top:8px">${caps.map(([c, n]) => `<button class="chip" type="button" data-cap="${c ?? ""}">${n}</button>`).join("")}</div></div>
      <p class="sub oc-tip">Ganho típico numa RTX 40: núcleo +100 a +200 MHz e memória +500 a +1000 MHz, subindo aos poucos.
        <b class="fg">Undervolt</b>: suba o offset do núcleo e ponha um teto; a placa chega ao mesmo clock com menos tensão, esquenta menos e gasta menos.</p>
      <div class="row"><button class="btn" id="apply" type="button">Aplicar</button><button class="btn outline" id="reset" type="button">Restaurar padrão</button>
        <span class="sub" id="diff"></span></div>
      <div class="status" id="tuneMsg">Valem até reiniciar o PC. Se travar, reinicie: a placa volta ao padrão de fábrica.</div>`;
    syncTune();
    $("power").addEventListener("input", (e) => ((want.power_w = +e.target.value), syncTune()));
    $("core")?.addEventListener("input", (e) => ((want.core_offset = +e.target.value), syncTune()));
    $("mem")?.addEventListener("input", (e) => ((want.mem_offset = +e.target.value), syncTune()));
    $("caps").addEventListener("click", (e) => {
      const c = e.target.closest("[data-cap]");
      if (!c) return;
      want.clock_cap = c.dataset.cap ? +c.dataset.cap : null;
      syncTune();
    });
    $("apply").addEventListener("click", () => apply(false));
    $("reset").addEventListener("click", () => apply(true));
  }

  function syncTune() {
    $("power").value = want.power_w;
    $("powerVal").innerHTML = `${num(want.power_w)}<u>W</u>`;
    if ($("core")) {
      $("core").value = want.core_offset;
      $("coreVal").innerHTML = `${signed(want.core_offset)}<u>MHz</u>`;
    }
    if ($("mem")) {
      $("mem").value = want.mem_offset;
      $("memVal").innerHTML = `${signed(want.mem_offset)}<u>MHz</u>`;
    }
    el.querySelectorAll("#caps .chip").forEach((c) => c.classList.toggle("on", (c.dataset.cap ? +c.dataset.cap : null) === want.clock_cap));
    const ch = changes();
    $("diff").textContent = ch.length ? `Muda: ${ch.join(" · ")}` : "Nada mudou.";
    $("apply").disabled = busy || !ch.length;
    $("reset").disabled = busy;
    // passou do padrão: avisa no próprio controle
    el.querySelectorAll(".oc-slider").forEach((s) => s.classList.remove("hot"));
    if (want.core_offset > 150) $("core")?.closest(".oc-slider").classList.add("hot");
    if (want.mem_offset > 1000) $("mem")?.closest(".oc-slider").classList.add("hot");
  }

  async function apply(reset) {
    busy = true;
    syncTune();
    const target = reset ? { power_w: Math.round(L.power_default_w), core_offset: 0, mem_offset: 0, clock_cap: null } : { ...want };
    const r = await runJob(
      () => (reset ? call("oc", "reset", { index: g.index }) : call("oc", "apply", { index: g.index, ...target })),
      $("job"),
      reset ? "Voltando a GPU ao padrão" : "Aplicando ajustes na GPU",
    );
    busy = false;
    g = (await call("oc", "gpu", { index: g.index })) ?? g;
    if (r.ok) {
      appliedCap = target.clock_cap;
      now = { ...target };
      want = { ...target };
      $("tuneMsg").textContent = reset ? "GPU de volta ao padrão de fábrica." : "Aplicado. Rode o verificador para confirmar a estabilidade.";
    } else {
      // pode ter aplicado parte: relê o que o driver diz
      now = { power_w: Math.round(g.power_limit_w ?? now.power_w), core_offset: g.core_offset?.current ?? 0, mem_offset: g.mem_offset?.current ?? 0, clock_cap: now.clock_cap };
      $("tuneMsg").textContent = r.message;
    }
    syncTune();
    paintLive();
  }

  // ---------------- verificador ----------------
  let target = store("target") ?? "cpu";
  if (target !== "cpu" && target !== "watch" && !info.gpu_load) target = "cpu";
  let minutes = store("minutes") ?? 5;
  let wasRunning = false;

  function paintCheck() {
    const gpuOk = info.gpu_load;
    $("check").innerHTML = `
      <label class="field" style="margin-top:0">Configuração testada</label>
      <input type="text" id="label" style="width:100%" placeholder="ex.: GPU 125 W, núcleo +150, memória +800 · Curve Optimizer -15">
      <div class="oc-pick">
        <div><label class="field">Testar</label><div class="chips" id="targets">${TARGETS.map(([k, n]) => {
          const off = (k === "gpu" || k === "both") && !gpuOk;
          return `<button class="chip" type="button" data-target="${k}" ${off ? 'disabled title="O teste de carga da GPU (reset-gpu) ainda não está disponível nesta versão. Use Só observar com um jogo aberto."' : ""}>${n}</button>`;
        }).join("")}</div></div>
        <div><label class="field">Duração</label><div class="chips" id="durs">${DURATIONS.map((d) => `<button class="chip" type="button" data-min="${d}">${d} min</button>`).join("")}</div></div>
      </div>
      <p class="sub" id="targetHelp"></p>
      <div class="row"><button class="btn" id="run" type="button">Iniciar verificação</button><span class="spacer"></span><span class="sub mono" id="clock"></span></div>
      <div class="oc-progress"><i id="prog"></i></div>
      <div class="oc-live" id="live"></div>
      <div id="verdict"></div>
      <div id="history"></div>`;
    $("label").value = store("label") ?? "";
    $("label").addEventListener("input", (e) => store("label", e.target.value));
    $("targets").addEventListener("click", (e) => {
      const c = e.target.closest("[data-target]");
      if (c && !c.disabled) (target = c.dataset.target), store("target", target), syncPick();
    });
    $("durs").addEventListener("click", (e) => {
      const c = e.target.closest("[data-min]");
      if (c) (minutes = +c.dataset.min), store("minutes", minutes), syncPick();
    });
    $("run").addEventListener("click", async () => {
      if (wasRunning) return call("oc", "check_stop");
      try {
        $("verdict").innerHTML = "";
        await call("oc", "check_start", { target, minutes, index: g?.index ?? 0 });
        pollCheck();
      } catch (e) {
        $("live").textContent = String(e);
      }
    });
    syncPick();
    paintHistory();
  }

  function syncPick() {
    el.querySelectorAll("#targets .chip").forEach((c) => c.classList.toggle("on", c.dataset.target === target));
    el.querySelectorAll("#durs .chip").forEach((c) => c.classList.toggle("on", +c.dataset.min === minutes));
    $("targetHelp").textContent = {
      cpu: `Carga em todos os núcleos e teste da RAM (escreve e confere um padrão). Para a ${info.limits_c.cpu} °C.`,
      gpu: `Cena pesada na GPU numa janela (reset-gpu) enquanto o app observa clock, temperatura e limitadores. Para a ${info.limits_c.gpu} °C.`,
      both: "CPU, RAM e GPU ao mesmo tempo: o pior caso para a fonte e a refrigeração.",
      watch: "Sem carga do app: abra o jogo ou benchmark que costuma travar e o app observa clock, temperatura, limitadores e erros do driver.",
    }[target];
  }

  function paintRunning(st) {
    const p = st.progress;
    const running = st.running;
    $("run").textContent = running ? "Cancelar" : "Iniciar verificação";
    $("run").classList.toggle("outline", running);
    el.querySelectorAll("#targets .chip, #durs .chip").forEach((c) => (c.style.pointerEvents = running ? "none" : ""));
    if (!p) {
      $("prog").style.width = "0";
      $("live").innerHTML = running ? `<span class="sub">Preparando…</span>` : "";
      $("clock").textContent = "";
      return;
    }
    const mmss = (s) => `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
    $("prog").style.width = running ? `${(100 * p.elapsed_s) / p.total_s}%` : "0";
    $("clock").textContent = running ? `${mmss(p.elapsed_s)} de ${mmss(p.total_s)}` : "";
    if (!running) return;
    const n = p.now;
    const cap = n.perfcap.filter((x) => x !== "Ocioso");
    const cell = (k, v) => `<div><span>${k}</span><b>${v}</b></div>`;
    $("live").innerHTML = `<div class="oc-live-head">${esc(p.phase)}</div><div class="oc-cells">
      ${target === "cpu" ? "" : cell("GPU", `${fmtTemp(n.gpu_temp)} · ${fmtMhz(n.gpu_core)}`)}
      ${target === "cpu" ? "" : cell("Uso da GPU", n.gpu_load == null ? "—" : `${num(n.gpu_load)}%`)}
      ${target === "cpu" ? "" : cell("Limitador", cap.length ? esc(cap.join(", ")) : "nada")}
      ${cell("CPU", `${fmtTemp(n.cpu_temp)} · ${fmtMhz(n.cpu_mhz)}`)}
      ${cell("Máx. CPU / GPU", `${fmtTemp(p.max_cpu_temp)} / ${fmtTemp(p.max_gpu_temp)}`)}
      ${target === "cpu" || target === "both" ? cell("Erros de memória", num(p.mem_errors)) : ""}
      ${p.fps != null ? cell("FPS do teste", num(p.fps)) : ""}
    </div>`;
  }

  function paintVerdict(r) {
    if (!r) return;
    $("live").innerHTML = "";
    const label = $("label").value.trim() || "sem descrição";
    if (r.cancelled) {
      $("verdict").innerHTML = `<div class="status">Verificação cancelada.</div>`;
      return;
    }
    $("verdict").innerHTML = `<div class="oc-verdict ${r.stable ? "ok" : "bad"}">
      <div class="row"><b class="oc-v-title">${r.stable ? "Estável" : "Instável"}</b><span class="spacer"></span><span class="mono small">${esc(label)}</span></div>
      <p>${r.stable
        ? `Nenhum erro em ${Math.round(r.seconds / 60)} min de teste. Para Curve Optimizer, use o PC normalmente por um dia e confira os erros recentes.`
        : `Motivo: ${esc(r.reason)}. Volte um passo no ajuste e teste de novo.`}</p>
      ${r.notes.length ? `<ul class="oc-notes">${r.notes.map((n) => `<li>${esc(n)}</li>`).join("")}</ul>` : ""}
      ${kv([["Configuração", label], ...r.rows])}
    </div>`;
  }

  function saveHistory(r) {
    if (!r || r.cancelled) return;
    const label = $("label").value.trim() || "sem descrição";
    const h = store("history") ?? [];
    h.unshift({ at: Date.now(), stable: r.stable, label, target, minutes: Math.round(r.seconds / 60), reason: r.reason });
    store("history", h.slice(0, 12));
    paintHistory();
  }

  function paintHistory() {
    const h = store("history") ?? [];
    $("history").innerHTML = h.length
      ? `<label class="field">Histórico</label>${h.map((x) => `<div class="oc-hist"><span class="dot ${x.stable ? "ok" : "bad"}"></span>
          <span class="mono small faint">${new Date(x.at).toLocaleString("pt-BR", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" })}</span>
          <span class="ellipsis" title="${esc(x.reason ?? "")}">${esc(x.label)}</span><span class="spacer"></span>
          <span class="tag">${esc(TARGETS.find(([k]) => k === x.target)?.[1] ?? x.target)} · ${x.minutes} min</span>
          <b class="small">${x.stable ? "estável" : "INSTÁVEL"}</b></div>`).join("")}`
      : "";
  }

  async function pollCheck() {
    const st = await call("oc", "check_status");
    paintRunning(st);
    if (wasRunning && !st.running) {
      paintVerdict(st.result);
      saveHistory(st.result);
      loadErrors();
    }
    wasRunning = st.running;
  }

  // ---------------- erros, CPU e guia ----------------
  async function loadErrors() {
    const e = await call("oc", "errors");
    $("errors").innerHTML = e.error
      ? `<div class="check"><span class="dot warn"></span><div><b>Não deu para ler o log</b><div class="sub">${esc(e.error)}</div></div></div>`
      : e.gpu + e.mce === 0
        ? `<div class="check"><span class="dot ok"></span><div><b>Nenhum erro nos últimos 7 dias</b><div class="sub">Nem travamento do driver de vídeo (Xid) nem erro de hardware da CPU/RAM (MCE).</div></div></div>`
        : `<div class="check"><span class="dot bad"></span><div><b>${e.gpu} erro(s) da GPU · ${e.mce} de hardware (MCE)</b>
            <div class="sub">Últimos 7 dias. Último: <span class="mono">${esc(e.last ?? "—")}</span></div>
            <div class="sub" style="margin-top:4px">Com overclock ou Curve Optimizer ligado, isso indica instabilidade: volte um passo.</div></div></div>`;
  }

  function paintCpu() {
    const c = info.cpu;
    const amd = /amd/i.test(info.cpu_vendor) || /ryzen/i.test(info.cpu_model);
    $("cpu").innerHTML = `<div class="sub" style="margin-bottom:8px">${esc(info.cpu_model)}</div>${kv([
      ["Driver de frequência", c.driver && `${c.driver}${c.amd_pstate ? ` (modo ${c.amd_pstate})` : ""}`],
      ["Governor", c.governor && `${c.governor}${c.governors.length > 1 ? ` · opções: ${c.governors.join(", ")}` : ""}`],
      ["Preferência de energia", c.epp && `${c.epp}${c.epps.length > 1 ? ` · opções: ${c.epps.join(", ")}` : ""}`],
      ["Boost", c.boost == null ? null : c.boost ? "ligado" : "DESLIGADO"],
      ["Preferred core", c.prefcore == null ? null : c.prefcore ? "ligado (tarefas vão para os núcleos mais rápidos)" : "desligado"],
      ["Clock", c.max_mhz ? `${fmtMhz(c.min_mhz)} a ${fmtMhz(c.max_mhz)}` : null],
      ["Limite do sistema", c.limit_mhz && c.max_mhz && c.limit_mhz < c.max_mhz ? `${fmtMhz(c.limit_mhz)} (abaixo do máximo)` : null],
    ])}
    ${c.boost === false ? `<div class="test-banner">O boost está desligado: a CPU não passa do clock base. Ligue em Energia ou com <code>echo 1 | sudo tee /sys/devices/system/cpu/cpufreq/boost</code>.</div>` : ""}
    <p class="sub" style="margin-top:12px">${amd
      ? "PBO, Curve Optimizer, Eco Mode e tensões da CPU <b class=\"fg\">ficam na BIOS</b>: o Linux não tem um jeito seguro e oficial de mudar isso com o sistema rodando. Governor e boost são ajustes de energia, não overclock. Veja o guia abaixo."
      : "Limites de energia (PL1/PL2) e undervolt da CPU <b class=\"fg\">ficam na BIOS</b>. Governor e boost são ajustes de energia, não overclock. Veja o guia abaixo."}</p>`;
  }

  function paintGuide() {
    const b = info.board ?? {};
    const maker = b.vendor ?? "";
    const giga = /gigabyte/i.test(maker), asus = /asus/i.test(maker), msi = /micro-star|msi/i.test(maker);
    const ryzen = /ryzen/i.test(info.cpu_model);
    const cpuShort = info.cpu_model.replace(/^(AMD|Intel\(R\))\s+/i, "").replace(/\s+\d+-Core Processor$/i, "").replace(/\(R\)|\(TM\)/g, "");
    const boardName = [giga ? "Gigabyte" : msi ? "MSI" : asus ? "ASUS" : maker.split(" ")[0], b.model ?? b.product].filter(Boolean).join(" ");
    const pbo = giga ? "Tweaker → Advanced CPU Settings → Precision Boost Overdrive (ou Settings → AMD Overclocking)"
      : asus ? "Advanced → AMD Overclocking → Precision Boost Overdrive (ou AI Tweaker → PBO)"
      : msi ? "OC → Advanced CPU Configuration → AMD Overclocking → Precision Boost Overdrive"
      : "Advanced → AMD Overclocking → Precision Boost Overdrive";
    const expo = giga ? "Tweaker → Extreme Memory Profile (X.M.P.) → EXPO1" : asus ? "AI Tweaker → Ai Overclock Tuner → EXPO I" : msi ? "OC → A-XMP/EXPO → Profile 1" : "procure EXPO / XMP no menu de memória";
    const steps = [
      ["0", "Antes de começar", giga ? "Save & Exit → Save Profiles" : "Save & Exit → Save Profile", "Salve o perfil atual da BIOS. Se o PC não ligar depois de um ajuste, espere 3 tentativas (muitas placas voltam sozinhas) ou limpe a CMOS (jumper CLR_CMOS na placa ou tirando a bateria por 1 minuto)."],
      ["1", "Ative o perfil da memória (EXPO / XMP)", expo, "É o maior ganho em jogos para a maioria dos PCs: a RAM sai do padrão lento e vai para a velocidade da etiqueta. Teste: verificador \"CPU + RAM\" por 15 min (o teste de RAM acusa erro se o perfil não for estável)."],
      ...(ryzen
        ? [
            ["2", "Precision Boost Overdrive (PBO)", pbo, "Precision Boost Overdrive → Advanced. PBO Limits → Motherboard. Isso libera os limites de energia para o boost durar mais. Teste com o verificador."],
            ["3", "Curve Optimizer (undervolt da CPU)", pbo + " → Curve Optimizer", `All Cores → Negative → 10. Teste. Se passar, tente 15, 20, 25… até falhar; volte 5 a partir de onde falhou. No ${cpuShort} costuma ficar entre -15 e -30. Instabilidade de Curve Optimizer aparece muito em uso leve: use o PC normalmente por um dia e veja "Erros de hardware recentes" aqui (erro MCE = volte um passo).`],
            ["4", "Max CPU Boost Clock Override (opcional)", pbo, "Depois do Curve Optimizer estável: +100 MHz, teste; depois +200 MHz. Ganho pequeno; pode exigir reduzir o Curve Optimizer."],
            ["5", "Alternativa fria: Eco Mode", pbo + " → Eco Mode", "Em vez de mais desempenho: limita a CPU a 65 W (ou 105 W). Perde ~5–10% em tarefas pesadas multi-core, quase nada em jogos, e roda bem mais frio e silencioso."],
          ]
        : [["2", "Processador Intel", "Tweaker / Ai Tweaker / OC → CPU", "Para Intel, prefira undervolt (offset negativo de tensão) e limites de energia (PL1/PL2) em vez de aumentar multiplicadores. Teste cada passo com o verificador."]]),
    ];
    $("guide").innerHTML = `<h2>Guia: overclock na BIOS${boardName ? ` · ${esc(boardName)}` : ""} · ${esc(cpuShort)}</h2>
      <p class="sub">Faça uma mudança por vez e rode o verificador depois de cada uma. Os menus podem mudar um pouco com a versão da BIOS.
        Valores são pontos de partida seguros, não garantia: cada chip é diferente.</p>
      <div class="oc-steps">${steps.map(([n, t, w, d]) => `<div class="oc-step"><span class="oc-num">${n}</span><div>
        <b>${esc(t)}</b><div class="mono small faint" style="margin:2px 0 5px">${esc(w)}</div><div class="sub">${esc(d)}</div></div></div>`).join("")}</div>`;
  }

  paintLive();
  paintTune();
  paintCheck();
  paintCpu();
  paintGuide();
  loadErrors();
  // um teste começado antes de sair da tela continua aparecendo (e o último resultado também)
  const st0 = await call("oc", "check_status");
  paintRunning(st0);
  if (!st0.running) paintVerdict(st0.result);
  wasRunning = st0.running;

  return every(1000, async () => {
    if (g && !busy) {
      g = (await call("oc", "gpu", { index: g.index })) ?? g;
      paintLive();
    }
    if (wasRunning) await pollCheck();
  });
}
