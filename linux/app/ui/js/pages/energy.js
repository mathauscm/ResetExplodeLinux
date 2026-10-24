// Energia: fonte e bateria, perfil de energia (power-profiles-daemon), política de clock da CPU (governor, EPP,
// boost) e consumo/limites da GPU. Perfil troca na hora sem senha; mudanças na CPU pedem a senha (pkexec).

import { call, head, kv, esc, num, fmtW, bar, every } from "../lib.js";
import { runJob } from "../pkgjob.js";

const PROFILE_TEXT = {
  performance: "Clock máximo para jogos e benchmarks",
  balanced: "Padrão: acelera quando precisa",
  "power-saver": "Menos calor, ruído e consumo",
};

// governor: [nome curto, explicação]. Com amd-pstate/intel_pstate ativos, "powersave" não é lento: quem decide é o EPP.
function govText(g, hwManaged) {
  const t = {
    performance: ["Desempenho", "Clock alto o tempo todo: resposta máxima, mais consumo parado."],
    powersave: hwManaged
      ? ["Pelo EPP", "O próprio processador escolhe o clock, guiado pela preferência abaixo. Não deixa o PC lento."]
      : ["Economia", "Clock mínimo o tempo todo. Deixa o PC lento."],
    schedutil: ["Automático", "O kernel ajusta o clock conforme a carga. É o padrão da maioria das distros."],
    ondemand: ["Sob demanda", "Sobe o clock quando a carga passa de um limite."],
    conservative: ["Conservador", "Como o sob demanda, mas sobe o clock aos poucos."],
    userspace: ["Manual", "Clock fixo escolhido por um programa."],
  };
  return t[g] ?? [g, ""];
}

const EPP_TEXT = {
  default: "Padrão do firmware",
  performance: "Desempenho",
  balance_performance: "Equilibrado, puxando para desempenho",
  balance_power: "Equilibrado, puxando para economia",
  power: "Economia",
};

const ghz = (mhz) => (mhz == null ? "—" : `${num(mhz / 1000, 2)} GHz`);
const PLUG = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 2v6M15 2v6M6 8h12v4a6 6 0 0 1-12 0zM12 18v4"/></svg>`;

export async function render(el) {
  el.innerHTML =
    head("RESET EXPLODE", "Energia", `<span class="sub" id="stamp"></span><button class="btn outline" id="refresh" type="button">Atualizar</button>`) +
    `<div class="grid en-top" id="top"><div class="card"><div class="sub">Lendo bateria e energia…</div></div></div>
    <div class="card en-cpu" id="cpu" hidden></div>
    <div class="card job" id="job" hidden style="margin-top:12px"></div>
    <p class="faint small" style="margin-top:14px">Os dados vêm do kernel (<code>/sys/class/power_supply</code>, <code>cpufreq</code>) e do driver NVIDIA.
      O consumo da fonte (PSU) do PC não é medido; os valores são dos sensores de CPU e GPU.</p>`;

  const $ = (id) => el.querySelector("#" + id);
  let info = null;
  let live = null;
  let busy = false;
  let sourceDrawn = false;
  const gpuHist = [];

  async function load() {
    $("refresh").disabled = true;
    try {
      info = await call("energy", "overview");
      draw();
      $("stamp").textContent = `${info.source.batteries.length ? "Bateria detectada" : "Nenhuma bateria (PC de mesa)"} · lido às ${new Date().toLocaleTimeString("pt-BR")}`;
    } catch (e) {
      $("top").innerHTML = `<div class="card"><div class="check"><span class="dot bad"></span><div><b>Não deu para ler</b><div class="sub">${esc(String(e))}</div></div></div></div>`;
    } finally {
      $("refresh").disabled = false;
    }
  }

  // ---------------------------------------------------------------- cartões de cima

  function sourceCard() {
    const s = info.source;
    const cpuW = live?.cpu?.power_w, gpuW = live?.gpus?.[0]?.power_w;
    const total = cpuW != null && gpuW != null ? cpuW + gpuW : null;
    const devices = s.devices.length
      ? `<label class="field">Periféricos</label>${s.devices
          .map((d) => `<div class="row small"><span>${esc(d.model ?? d.name)}</span><span class="spacer"></span><b class="mono">${d.capacity_pct != null ? num(d.capacity_pct) + "%" : esc(d.level ?? "—")}</b></div>`)
          .join("")}`
      : "";
    return `<div class="card"><h2>Fonte de energia</h2>
      <div class="en-source"><span class="en-plug">${PLUG}</span><div><div class="en-source-name">${esc(s.source.replace(" (PC de mesa)", ""))}</div>
        <div class="sub">${s.batteries.length ? (s.ac_online === false ? "Rodando na bateria" : "Ligado na tomada") : "PC de mesa · nenhuma bateria, tudo vem direto da fonte"}</div></div></div>
      <label class="field">Consumo medido agora</label>
      <dl class="kv">
        <dt>Processador</dt><dd id="cpuW">${live?.cpu ? (live.cpu.power_w != null ? fmtW(live.cpu.power_w) : `<span class="faint">não liberado</span>`) : "—"}</dd>
        <dt>Placa de vídeo</dt><dd id="gpuW2">${gpuW != null ? fmtW(gpuW) : "—"}</dd>
        ${total != null ? `<dt>CPU + GPU</dt><dd>${fmtW(total)}</dd>` : ""}
      </dl>
      ${live?.cpu?.note ? `<div class="sub" style="margin-top:8px">${esc(live.cpu.note)}</div>` : ""}
      ${devices}</div>`;
  }

  function batteryCard(b, many) {
    const dot = { good: "ok", warn: "warn", bad: "bad" }[b.verdict] ?? "";
    const h = b.minutes_left != null ? `${Math.floor(b.minutes_left / 60)} h ${String(b.minutes_left % 60).padStart(2, "0")} min` : null;
    return `<div class="card"><div class="row"><h2 style="margin:0">${many ? esc(b.name) : "Bateria"}</h2><span class="spacer"></span>
        <span class="tag"><span class="dot ${dot} en-dot"></span>${esc(b.label)}</span></div>
      <div class="big">${b.capacity_pct == null ? "—" : `${num(b.capacity_pct)}<small>%</small>`}</div>
      ${bar(b.capacity_pct, b.capacity_pct != null && b.capacity_pct < 20 && b.discharging)}
      <div class="sub" style="margin:8px 0 10px">${esc([b.status, h && (b.charging ? `cheia em ${h}` : `resta ${h}`)].filter(Boolean).join(" · "))}</div>
      ${kv([
        ["Capacidade de projeto", b.design_wh && `${num(b.design_wh, 1)} Wh`],
        ["Capacidade máxima atual", b.full_wh && `${num(b.full_wh, 1)} Wh`],
        ["Desgaste", b.wear_pct != null && `${num(b.wear_pct, 1)}%`],
        ["Ciclos de carga", b.cycles],
        [b.charging ? "Potência de carga" : "Consumo atual", b.power_w && `${num(b.power_w, 1)} W`],
        ["Tensão", b.voltage_v && `${num(b.voltage_v, 2)} V`],
        ["Fabricante", b.manufacturer],
        ["Modelo", b.model],
        ["Química", b.technology],
      ])}
      <p class="sub" style="margin-top:10px">${esc(b.reasons.join(" "))}</p></div>`;
  }

  function profileCard() {
    const p = info.profiles;
    if (!p.available) {
      return `<div class="card"><h2>Perfil de energia</h2><div class="check"><span class="dot"></span><div><b>Indisponível</b><div class="sub">${esc(p.note ?? "")}</div></div></div></div>`;
    }
    const act = p.profiles.find((x) => x.active);
    const drivers = [act?.cpu_driver && `CPU: ${act.cpu_driver}`, act?.platform_driver && `plataforma: ${act.platform_driver}`].filter(Boolean).join(" · ");
    const degraded = p.profiles.find((x) => x.degraded);
    return `<div class="card"><h2>Perfil de energia</h2>
      <div class="en-seg" id="profiles">${p.profiles
        .map((x) => `<button type="button" class="${x.active ? "on" : ""}" data-profile="${esc(x.id)}" ${busy ? "disabled" : ""}><b>${esc(x.label)}</b><span>${esc(PROFILE_TEXT[x.id] ?? "")}</span></button>`)
        .join("")}</div>
      <div class="sub" style="margin-top:10px">${esc(drivers)}${drivers ? " · " : ""}troca na hora, sem senha</div>
      ${degraded ? `<div class="test-banner">O perfil ${esc(degraded.label)} está rebaixado pelo sistema (${esc(degraded.degraded)}).</div>` : ""}
      ${p.active && p.active !== "performance" ? `<div class="test-banner">Para jogos e benchmarks, o perfil <b>Desempenho</b> evita que o PC diminua o clock.</div>` : ""}
      <div class="status" id="profMsg"></div></div>`;
  }

  function gpuCard() {
    const g = live?.gpus?.[0] ?? info.gpus[0];
    if (!g) return `<div class="card"><h2>Placa de vídeo</h2><div class="sub">Nenhuma GPU NVIDIA encontrada pelo NVML. Placas AMD e Intel entram numa próxima etapa.</div></div>`;
    return `<div class="card"><h2>Placa de vídeo</h2><div class="sub">${esc(g.name)}</div>
      <div class="big" id="gpuW">—</div>
      <div id="gpuBar"></div>
      <canvas class="en-spark" id="spark"></canvas>
      <div class="stat-row"><div>Limite atual<b>${fmtW(g.limit_w)}</b></div><div>Padrão<b>${fmtW(g.default_w)}</b></div>
        <div>Ajustável<b>${g.min_w != null ? `${num(g.min_w)}–${num(g.max_w)} W` : "—"}</b></div></div>
      <div class="sub" id="gpuNote" style="margin-top:10px"></div></div>`;
  }

  function updateGpu() {
    const g = live?.gpus?.[0];
    if (!g || !$("gpuW")) return;
    $("gpuW").innerHTML = g.power_w == null ? "—" : `${num(g.power_w, 1)}<small>W</small>`;
    $("gpuBar").innerHTML = bar(g.power_w != null && g.limit_w ? (100 * g.power_w) / g.limit_w : 0);
    const state = [g.pstate, g.perfcap.length ? `limitada por: ${g.perfcap.join(", ").toLowerCase()}` : null].filter(Boolean).join(" · ");
    $("gpuNote").textContent =
      g.power_w == null
        ? "Esta placa não informa o consumo ao driver (NVML)."
        : (g.power_from === "amostras" ? "Média das amostras do driver (esta placa não dá a leitura instantânea). " : "") + (state ? `Estado: ${state}.` : "");
    spark($("spark"), gpuHist, g.limit_w);
  }

  // ---------------------------------------------------------------- processador

  function cpuCard() {
    const c = info.cpu;
    if (!c) return "";
    const hwManaged = /pstate/.test(c.driver ?? "") && c.driver_mode === "active";
    const lock = busy ? "disabled" : "";
    const gov = c.governor ? govText(c.governor, hwManaged) : null;
    const chips = (kind, list, cur, label, extra = "") =>
      `<div class="chips">${list.map((v) => `<button class="chip${v === cur ? " on" : ""}" type="button" data-kind="${kind}" data-value="${esc(v)}" ${lock} ${extra}>${esc(label(v))}</button>`).join("")}</div>`;
    const maxRank = Math.max(...c.policies.map((p) => p.prefcore_rank ?? 0));
    return `<div class="row"><h2 style="margin:0">Processador</h2><span class="spacer"></span>
        <span class="tag">${esc(c.driver ?? "cpufreq")}${c.driver_mode ? ` · ${esc(c.driver_mode)}` : ""}</span>
        ${c.prefcore ? `<span class="tag" title="O kernel manda as tarefas pesadas para os núcleos mais rápidos (ranking do firmware)">núcleo preferido</span>` : ""}</div>
      <div class="en-cpu-grid">
        <div>
          <label class="field">Governor</label>
          ${chips("governor", c.governors, c.governor, (g) => `${govText(g, hwManaged)[0]} · ${g}`)}
          <div class="sub" style="margin-top:6px">${gov ? esc(gov[1]) : "Os núcleos estão com governors diferentes."}</div>
          ${c.epps.length ? `<label class="field">Preferência de energia (EPP)</label>
            ${chips("epp", c.epps, c.epp, (e) => EPP_TEXT[e] ?? e, c.epp_locked ? "disabled" : "")}
            <div class="sub" style="margin-top:6px">${c.epp_locked ? "Com o governor <b>performance</b> o amd-pstate trava o EPP em desempenho. Use o governor <b>powersave</b> para escolher." : "Diz ao processador o quanto priorizar desempenho ou economia."}</div>` : ""}
          ${c.boost != null ? `<label class="field">Boost</label>
            <div class="chips">${[[true, "Ligado"], [false, "Desligado"]].map(([v, n]) => `<button class="chip${c.boost === v ? " on" : ""}" type="button" data-kind="boost" data-value="${v}" ${lock}>${n}</button>`).join("")}</div>
            <div class="sub" style="margin-top:6px">${c.boost ? "O processador passa do clock base quando há folga de temperatura e energia." : "Desligado: menos calor e consumo, mas perde desempenho."}</div>` : ""}
          <p class="faint small" style="margin-top:14px">Pede a senha de administrador e vale até reiniciar. Trocar o perfil de energia também muda estes valores.</p>
        </div>
        <div>
          <label class="field">Clock por núcleo agora</label>
          <div class="en-cores" id="cores">${c.policies
            .map((p) => `<div class="en-core${p.prefcore_rank && p.prefcore_rank === maxRank ? " pref" : ""}" title="CPU ${esc(p.cpus)}${p.prefcore_rank ? ` · ranking ${p.prefcore_rank}` : ""}"><div class="en-col"><i></i></div><span>${esc(p.cpus)}</span></div>`)
            .join("")}</div>
          <div class="stat-row"><div>Média<b id="clkAvg">—</b></div><div>Mais alto<b id="clkMax">—</b></div><div>Faixa do chip<b>${num(c.hw_min_mhz / 1000, 1)}–${num(c.hw_max_mhz / 1000, 2)} GHz</b></div></div>
          ${c.prefcore ? `<div class="faint small" style="margin-top:8px">Os núcleos marcados são os preferidos (os mais rápidos deste chip).</div>` : ""}
        </div>
      </div>
      <details class="adv"><summary>Detalhes por núcleo</summary>
        <table style="margin-top:8px"><thead><tr><th>CPU</th><th>GOVERNOR</th><th>EPP</th><th class="num">MÍN.</th><th class="num">MÁX.</th><th class="num">RANKING</th></tr></thead><tbody>${c.policies
          .map((p) => `<tr><td class="mono">${esc(p.cpus)}</td><td>${esc(p.governor ?? "—")}</td><td>${esc(p.epp ?? "—")}</td><td class="num">${ghz(p.min_mhz)}</td><td class="num">${ghz(p.max_mhz)}</td><td class="num">${p.prefcore_rank ?? "—"}</td></tr>`)
          .join("")}</tbody></table></details>`;
  }

  function updateClocks() {
    const cl = live?.clocks, c = info?.cpu;
    if (!cl?.length || !c || !$("cores")) return;
    const top = c.hw_max_mhz || Math.max(...cl);
    $("cores").querySelectorAll(".en-col i").forEach((i, k) => (i.style.height = `${Math.min(100, (100 * (cl[k] ?? 0)) / top)}%`));
    $("cores").querySelectorAll(".en-core").forEach((d, k) => (d.dataset.mhz = cl[k]));
    $("clkAvg").textContent = ghz(cl.reduce((a, b) => a + b, 0) / cl.length);
    $("clkMax").textContent = ghz(Math.max(...cl));
  }

  function draw() {
    if (!info) return;
    const s = info.source;
    $("top").innerHTML = [...s.batteries.map((b) => batteryCard(b, s.batteries.length > 1)), sourceCard(), profileCard(), gpuCard()].join("");
    $("cpu").hidden = !info.cpu;
    $("cpu").innerHTML = cpuCard();
    updateGpu();
    updateClocks();
  }

  // ---------------------------------------------------------------- ações

  el.addEventListener("click", async (e) => {
    const prof = e.target.closest("[data-profile]");
    if (prof && !prof.classList.contains("on")) {
      busy = true;
      $("profMsg").textContent = "Trocando o perfil…";
      try {
        info.profiles = await call("energy", "set_profile", { id: prof.dataset.profile });
        busy = false;
        // o daemon também muda governor/EPP: relê tudo
        await load();
      } catch (err) {
        busy = false;
        draw();
        $("profMsg").textContent = `Não deu para trocar: ${err}`;
      }
      return;
    }
    const chip = e.target.closest("[data-kind]");
    if (!chip || chip.classList.contains("on") || busy) return;
    const kind = chip.dataset.kind;
    const value = kind === "boost" ? chip.dataset.value === "true" : chip.dataset.value;
    const what = { governor: `governor ${value}`, epp: `EPP ${value}`, boost: value ? "ligar o boost" : "desligar o boost" }[kind];
    busy = true;
    draw();
    await runJob(() => call("energy", "cpu_set", { kind, value }), $("job"), `Processador: ${what}`);
    busy = false;
    await load();
  });
  $("refresh").addEventListener("click", load);

  await load();
  return every(2000, async () => {
    live = await call("energy", "live");
    const w = live.gpus?.[0]?.power_w;
    if (w != null) {
      gpuHist.push(w);
      if (gpuHist.length > 60) gpuHist.shift();
    }
    if (!info) return;
    updateGpu();
    updateClocks();
    // consumo da CPU/GPU no cartão da fonte
    const cpuW = $("cpuW"), gpuW2 = $("gpuW2");
    if (cpuW && live.cpu?.power_w != null) cpuW.textContent = fmtW(live.cpu.power_w);
    if (gpuW2) gpuW2.textContent = fmtW(w);
    if (!sourceDrawn && live.cpu) {
      sourceDrawn = true;
      draw(); // a primeira leitura traz o aviso do RAPL
    }
  });
}

/** Linha do consumo da GPU (últimos 2 min), com o limite como teto e leitura ao passar o mouse. */
function spark(cv, data, ceil) {
  if (!cv) return;
  const style = getComputedStyle(document.documentElement);
  const css = { getPropertyValue: (v) => style.getPropertyValue(v).trim() };
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth, h = cv.clientHeight;
  cv.width = w * dpr;
  cv.height = h * dpr;
  const ctx = cv.getContext("2d");
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, w, h);
  const top = Math.max(ceil || 0, ...data, 1) * 1.05;
  // o limite de energia como teto de referência
  if (ceil) {
    const yl = Math.round(h - 2 - ((h - 4) * ceil) / top) + 0.5;
    ctx.strokeStyle = css.getPropertyValue("--line-strong");
    ctx.setLineDash([3, 3]);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, yl);
    ctx.lineTo(w, yl);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.fillStyle = css.getPropertyValue("--muted");
    ctx.font = `10px ${css.getPropertyValue("--mono")}`;
    ctx.textBaseline = "top";
    ctx.fillText(`limite ${num(ceil)} W · últimos 2 min`, 0, yl + 4);
  }
  if (data.length < 2) return;
  const x = (i) => (w * i) / 59 + (w * (60 - data.length)) / 59;
  const y = (v) => h - 2 - ((h - 4) * v) / top;
  ctx.fillStyle = css.getPropertyValue("--hover");
  ctx.beginPath();
  ctx.moveTo(x(0), h);
  data.forEach((v, i) => ctx.lineTo(x(i), y(v)));
  ctx.lineTo(x(data.length - 1), h);
  ctx.fill();
  ctx.strokeStyle = css.getPropertyValue("--fg");
  ctx.lineWidth = 2;
  ctx.lineJoin = "round";
  ctx.beginPath();
  data.forEach((v, i) => (i ? ctx.lineTo(x(i), y(v)) : ctx.moveTo(x(i), y(v))));
  ctx.stroke();
  cv.title = `Consumo da GPU nos últimos ${data.length * 2} s: mín. ${num(Math.min(...data), 1)} W · máx. ${num(Math.max(...data), 1)} W`;
}
