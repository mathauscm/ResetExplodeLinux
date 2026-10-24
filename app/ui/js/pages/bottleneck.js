// Gargalo: o que está limitando o FPS. Equilíbrio teórico GPU × CPU, veredito ao vivo (1/s) e uma análise de sessão
// de 1, 3 ou 5 minutos que roda no backend (continua mesmo se você trocar de tela) e manda o progresso por evento.

import { call, listen, head, esc, num, fmtTemp, bar, every } from "../lib.js";

const pct = (v) => (v == null ? "—" : `${num(v)}%`);
const mmss = (s) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

function meter(label, value, fill, hot = false) {
  return `<div class="bn-meter"><div class="bn-meter-top"><span>${esc(label)}</span><b>${value}</b></div>${bar(fill, hot)}</div>`;
}

export async function render(el) {
  let minutes = 1;
  let running = null; // {elapsed, total}

  el.innerHTML = head("RESET JOGOS", "Gargalo") + `
    <p class="sub bn-sub">Descobre o que está limitando o seu FPS: GPU, CPU, memória ou temperatura.</p>
    <div class="bn-top">
      <div class="card" id="balance"><h2>Equilíbrio do hardware</h2><div class="sub">Lendo o hardware…</div></div>
      <div class="card bn-verdict" id="verdict">
        <div class="bn-label">Agora</div>
        <div class="bn-title" id="vTitle">—</div>
        <div class="bn-game" id="vGame"></div>
        <p class="bn-advice" id="vAdvice"></p>
        <div class="bn-rules">Regras, na ordem: VRAM ≥ 97% · temperatura · GPU ≥ 90% · CPU ou um núcleo ≥ 85% · RAM ≥ 92%</div>
      </div>
    </div>
    <div class="card bn-live"><div class="row"><h2 style="margin:0">Leituras ao vivo</h2><span class="spacer"></span><span class="faint small" id="perfcap"></span></div>
      <div class="bn-meters" id="meters"></div></div>
    <div class="card bn-session">
      <div class="row bn-session-head">
        <div style="flex:1;min-width:240px"><h2>Análise de sessão</h2>
          <div class="sub">Clique em iniciar, abra o jogo e jogue normalmente. Ao final você recebe o diagnóstico. A análise continua mesmo se você trocar de tela.</div></div>
        <div class="chips" id="dur">${[1, 3, 5].map((m) => `<button class="chip${m === minutes ? " on" : ""}" type="button" data-min="${m}">${m} min</button>`).join("")}</div>
        <button class="btn" id="go" type="button">Iniciar análise</button>
      </div>
      <div class="bn-progress" id="progress" hidden>${bar(0)}<div class="status" id="pStatus"></div></div>
      <div class="status" id="sStatus"></div>
      <div id="result"></div>
    </div>`;
  const $ = (id) => el.querySelector("#" + id);

  // ---------------- equilíbrio ----------------
  call("games", "bn_init").then(({ balance: b, machine: m }) => {
    $("balance").innerHTML = `<h2>Equilíbrio do hardware</h2>
      <div class="bn-names"><div><span class="faint small">CPU</span><b>${esc(m.cpu_short)}</b><span class="faint small">índice ${num(m.cpu_idx)}</span></div>
        <div style="text-align:right"><span class="faint small">GPU</span><b>${esc(m.gpu_short)}</b><span class="faint small">índice ${num(m.gpu_idx)}</span></div></div>
      <div class="bn-scale"><i class="bn-mid"></i><span class="bn-dot" style="left:${(b.pos * 100).toFixed(1)}%"></span></div>
      <div class="bn-scale-labels"><span>limita a GPU</span><span>equilibrado</span><span>limita a CPU</span></div>
      <p class="sub" style="margin-top:12px">${esc(b.text)}</p>
      <div class="faint small">GPU ÷ CPU = ${num(b.ratio, 2)}× (RTX 4070 = 100; Ryzen 7 7700X = 100)</div>`;
  });

  // ---------------- ao vivo ----------------
  async function live() {
    const r = await call("games", "bn_live");
    const s = r.sample;
    $("verdict").classList.toggle("alert", r.alert);
    $("vTitle").textContent = r.title;
    $("vAdvice").textContent = r.advice;
    $("vGame").textContent = r.game ? `Jogo rodando: ${r.game}` : "Nenhum jogo da biblioteca rodando agora";
    const temps = Math.max(s.gpu_temp ?? 0, s.cpu_temp ?? 0);
    $("meters").innerHTML = [
      meter("Carga da GPU", pct(s.gpu_load), s.gpu_load),
      meter("Carga total da CPU", pct(s.cpu_total), s.cpu_total),
      meter(s.busiest_core ? `Núcleo mais ocupado (${s.busiest_core})` : "Núcleo mais ocupado", pct(s.cpu_max_core), s.cpu_max_core, s.cpu_max_core >= 85),
      meter("VRAM em uso", pct(s.vram_pct), s.vram_pct, s.vram_pct >= 97),
      meter("RAM em uso", pct(s.ram_pct), s.ram_pct, s.ram_pct >= 92),
      meter("Temperaturas (GPU / CPU)", `${fmtTemp(s.gpu_temp)} / ${fmtTemp(s.cpu_temp)}`, temps, s.gpu_thermal_cap || s.cpu_temp >= 95),
    ].join("");
    $("perfcap").textContent = s.gpu_perfcap.length ? `Limitador do clock da GPU: ${s.gpu_perfcap.join(", ")}` : "";
  }

  // ---------------- sessão ----------------
  function drawRunning() {
    const on = !!running;
    $("go").textContent = on ? "Cancelar" : "Iniciar análise";
    $("go").className = on ? "btn outline" : "btn";
    $("dur").querySelectorAll(".chip").forEach((c) => (c.disabled = on));
    $("progress").hidden = !on;
    if (on) $("progress").querySelector(".bar > i").style.width = `${Math.min(100, (100 * running.elapsed) / running.total)}%`;
  }

  function drawReport(r) {
    if (!r) return ($("result").innerHTML = "");
    $("result").innerHTML = `<div class="bn-result">
      <div class="bn-result-title">${esc(r.title)}</div>
      <div class="chips bn-summary">${r.summary.map((x) => `<span class="tag">${esc(x)}</span>`).join("")}</div>
      <h2 style="margin-top:16px">% do tempo em cada limite</h2>
      <div class="bn-dist">${r.distribution.map((d) => meter(d.title, `${num(d.pct)}%`, d.pct, ["cpu", "vram", "ram", "thermal"].includes(d.limiter) && d.pct >= 20)).join("")}</div>
      <p class="bn-advice-big">${esc(r.advice)}</p>
      ${r.recommendations.length ? `<h2 style="margin-top:14px">Recomendações</h2><ul class="bn-recs">${r.recommendations.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>` : ""}
      <div class="faint small" style="margin-top:10px">${r.samples} amostras${r.games.length ? ` · jogo detectado: ${esc(r.games.join(", "))}` : " · nenhum jogo da biblioteca detectado"}</div>
    </div>`;
  }

  const unProgress = await listen("games-bottleneck", (p) => {
    running = { elapsed: p.elapsed, total: p.total };
    drawRunning();
    $("pStatus").textContent = `Coletando… ${mmss(p.elapsed)} de ${mmss(p.total)} · ${p.count} amostras · agora: ${p.title} · ${p.game ? `jogo detectado: ${p.game}` : "abra o jogo agora"}`;
  });
  const unDone = await listen("games-bottleneck-done", (p) => {
    running = null;
    drawRunning();
    $("sStatus").textContent = p.cancelled ? "Análise cancelada." : `Análise concluída às ${new Date().toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" })}.`;
    drawReport(p.report);
  });

  el.addEventListener("click", async (e) => {
    const c = e.target.closest("[data-min]");
    if (c && !running) {
      minutes = Number(c.dataset.min);
      $("dur").querySelectorAll(".chip").forEach((x) => x.classList.toggle("on", x === c));
      return;
    }
    if (e.target.id === "go") {
      if (running) return call("games", "bn_stop");
      try {
        const r = await call("games", "bn_start", { minutes });
        running = { elapsed: 0, total: r.total };
        $("sStatus").textContent = "";
        $("pStatus").textContent = "Coletando… abra o jogo agora.";
        drawReport(null);
        drawRunning();
      } catch (err) {
        $("sStatus").textContent = String(err);
      }
    }
  });

  // voltando para a tela no meio (ou depois) de uma análise
  const st = await call("games", "bn_status");
  running = st.running;
  drawRunning();
  if (running) $("pStatus").textContent = "Coletando…";
  else if (st.last) {
    $("sStatus").textContent = "Resultado da última análise:";
    drawReport(st.last);
  }

  const stop = every(1000, live);
  return () => {
    stop();
    unProgress();
    unDone();
  };
}
