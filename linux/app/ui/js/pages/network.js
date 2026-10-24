// Rede: conexão ativa, tráfego ao vivo, adaptadores e o diagnóstico de lag do app do Windows (latência, jitter e
// perda até o roteador e a internet, DNS, velocidade e bufferbloat). O teste roda no backend; o andamento chega
// pelo evento "network-progress" e fica guardado aqui, então dá para sair da tela e voltar no meio.

import { call, listen, head, esc, num, bytes, every } from "../lib.js";

// ---------------------------------------------------------------- estado do diagnóstico (vive enquanto o app estiver aberto)

const S = {
  listening: false,
  running: false,
  stage: "Pronto. O teste leva cerca de 30 segundos.",
  frac: 0,
  names: [],
  addresses: [],
  pings: [[], [], []],
  loaded: [],
  speed: { down: [], up: [] },
  dns: [],
  report: null,
  at: null,
  redraw: null,
};

function reset() {
  Object.assign(S, { running: true, stage: "Começando…", frac: 0, names: [], addresses: [], pings: [[], [], []], loaded: [], speed: { down: [], up: [] }, dns: [], report: null, at: null });
}

function onEvent(e) {
  switch (e.type) {
    case "stage":
      S.stage = e.text;
      S.frac = e.frac;
      break;
    case "targets":
      S.names = e.names;
      S.addresses = e.addresses;
      break;
    case "ping":
      S.pings[e.target]?.push([e.seq, e.ms]);
      break;
    case "loaded":
      S.loaded.push([e.seq, e.ms]);
      break;
    case "speed":
      S.speed[e.dir].push([e.t, e.mbps, e.avg]);
      break;
    case "dns":
      S.dns.push(e.result);
      break;
    case "done":
      S.running = false;
      S.report = e.report;
      S.at = new Date();
      S.stage = `Concluído às ${S.at.toLocaleTimeString("pt-BR")}.`;
      S.frac = 1;
      break;
    case "cancelled":
      S.running = false;
      S.stage = "Teste cancelado.";
      S.frac = 0;
      break;
    case "error":
      S.running = false;
      S.stage = `Falhou: ${e.message}`;
      S.frac = 0;
      break;
  }
  S.redraw?.(e.type);
}

// ---------------------------------------------------------------- gráfico de linhas (canvas) com mira e leitura

const DASH = [[], [6, 4], [1.5, 3.5]];
const INK = ["--fg", "--muted", "--muted"];

const legendKey = (i) =>
  `<svg width="22" height="8" viewBox="0 0 22 8"><line x1="1" y1="4" x2="21" y2="4" stroke="var(${INK[i]})" stroke-width="2" stroke-linecap="round" stroke-dasharray="${DASH[i].join(" ")}"/></svg>`;

function legend(names) {
  return `<div class="nt-legend">${names.map((n, i) => `<span>${legendKey(i)}${esc(n)}</span>`).join("")}</div>`;
}

/**
 * `series`: [{name, points: [[x, y], ...]}]; `x0`/`x1`: faixa do eixo X; `fmtX`/`fmtY`: texto dos eixos e da leitura.
 * Guarda as opções no próprio canvas para redesenhar com a mira ao passar o mouse.
 */
function lineChart(cv, opts) {
  if (!cv) return;
  cv._opts = opts;
  if (!cv._hooked) {
    cv._hooked = true;
    cv.addEventListener("pointermove", (ev) => {
      const r = cv.getBoundingClientRect();
      cv._hover = ev.clientX - r.left;
      paint(cv);
    });
    cv.addEventListener("pointerleave", () => {
      cv._hover = null;
      paint(cv);
    });
  }
  paint(cv);
}

function paint(cv) {
  const o = cv._opts;
  const css = getComputedStyle(document.documentElement);
  const color = (v) => css.getPropertyValue(v).trim();
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth, h = cv.clientHeight;
  if (!w || !h) return;
  cv.width = w * dpr;
  cv.height = h * dpr;
  const ctx = cv.getContext("2d");
  ctx.scale(dpr, dpr);
  ctx.font = `10px ${css.getPropertyValue("--mono")}`;
  const L = 40, R = 8, T = 8, B = 20;
  const all = o.series.flatMap((s) => s.points.map((p) => p[1]));
  const raw = Math.max(o.yMin ?? 0, ...all) * 1.12 || 1;
  // topo "redondo": 1, 2, 5 × 10^n
  const mag = 10 ** Math.floor(Math.log10(raw));
  const top = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((v) => v >= raw);
  const X = (x) => L + ((w - L - R) * (x - o.x0)) / (o.x1 - o.x0 || 1);
  const Y = (y) => T + (h - T - B) * (1 - y / top);

  // grade e eixos discretos
  ctx.strokeStyle = color("--line");
  ctx.fillStyle = color("--faint");
  ctx.lineWidth = 1;
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  for (let k = 0; k <= 4; k++) {
    const v = (top * k) / 4, y = Math.round(Y(v)) + 0.5;
    ctx.beginPath();
    ctx.moveTo(L, y);
    ctx.lineTo(w - R, y);
    ctx.stroke();
    ctx.fillText(o.fmtY(v, true), L - 6, y);
  }
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  for (const x of o.ticks ?? []) ctx.fillText(o.fmtX(x), X(x), h - B + 6);

  if (!all.length) {
    ctx.fillStyle = color("--muted");
    ctx.textBaseline = "middle";
    ctx.fillText(o.empty ?? "sem dados", (L + w) / 2, h / 2 - B / 2);
    return;
  }

  o.series.forEach((s, i) => {
    if (!s.points.length) return;
    ctx.strokeStyle = color(INK[i]);
    ctx.lineWidth = 2;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.setLineDash(DASH[i]);
    ctx.beginPath();
    s.points.forEach(([x, y], k) => (k ? ctx.lineTo(X(x), Y(y)) : ctx.moveTo(X(x), Y(y))));
    ctx.stroke();
    ctx.setLineDash([]);
    if (s.points.length === 1) {
      ctx.fillStyle = color(INK[i]);
      ctx.beginPath();
      ctx.arc(X(s.points[0][0]), Y(s.points[0][1]), 3, 0, 7);
      ctx.fill();
    }
  });

  // mira: acha o X mais próximo e mostra todas as séries nele
  const tip = cv.parentElement.querySelector(".nt-tip");
  if (cv._hover == null || cv._hover < L) {
    if (tip) tip.hidden = true;
    return;
  }
  const xs = [...new Set(o.series.flatMap((s) => s.points.map((p) => p[0])))];
  const target = o.x0 + ((cv._hover - L) / (w - L - R)) * (o.x1 - o.x0);
  const near = xs.reduce((a, b) => (Math.abs(b - target) < Math.abs(a - target) ? b : a), xs[0]);
  const px = Math.round(X(near)) + 0.5;
  ctx.strokeStyle = color("--line-strong");
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(px, T);
  ctx.lineTo(px, h - B);
  ctx.stroke();
  const rows = o.series
    .map((s, i) => {
      const p = s.points.reduce((a, b) => (Math.abs(b[0] - near) < Math.abs(a[0] - near) ? b : a), s.points[0]);
      if (!p || Math.abs(p[0] - near) > (o.snap ?? 0.5)) return "";
      ctx.fillStyle = color(INK[i]);
      ctx.beginPath();
      ctx.arc(X(p[0]), Y(p[1]), 3.5, 0, 7);
      ctx.fill();
      return `<div>${legendKey(i)}<b>${esc(o.fmtY(p[1]))}</b><span>${esc(s.name)}</span></div>`;
    })
    .join("");
  if (!tip) return;
  tip.innerHTML = `<div class="nt-tip-x">${esc(o.fmtX(near, true))}</div>${rows || `<div class="faint">sem resposta</div>`}`;
  tip.hidden = false;
  const left = px + 12 + tip.offsetWidth > w ? px - 12 - tip.offsetWidth : px + 12;
  tip.style.left = `${left}px`;
}

// ---------------------------------------------------------------- formatação

const ms = (v, d = 1) => (v == null ? "—" : `${num(v, v >= 100 ? 0 : d)} ms`);
const mbps = (v) => (v == null ? "—" : `${num(v, v >= 100 ? 0 : 1)} Mbps`);
function rate(bitsPerSec) {
  if (bitsPerSec == null) return ["—", ""];
  if (bitsPerSec < 1e6) return [num(bitsPerSec / 1e3, 0), "kbps"];
  return [num(bitsPerSec / 1e6, bitsPerSec >= 1e8 ? 0 : 1), "Mbps"];
}
// média móvel de 1 s: as leituras de 250 ms chegam em rajadas (buffers do curl e do kernel)
const smooth = (pts) => pts.map((p, i) => {
  const win = pts.slice(Math.max(0, i - 3), i + 1);
  return [p[0], win.reduce((a, q) => a + q[1], 0) / win.length];
});

const ICON = {
  ethernet: '<path d="M7 3h10v7h3v8H4v-8h3z"/><path d="M9 14v2M12 14v2M15 14v2"/>',
  wifi: '<path d="M2 9a15 15 0 0 1 20 0M5 13a10 10 0 0 1 14 0M8.5 16.5a5 5 0 0 1 7 0"/><circle cx="12" cy="20" r="1"/>',
  vpn: '<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
  other: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
};
const icon = (k) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${ICON[k] ?? ICON.other}</svg>`;

/** Quatro barrinhas de sinal do Wi-Fi. */
function signalBars(pct) {
  const n = pct == null ? 0 : pct >= 75 ? 4 : pct >= 50 ? 3 : pct >= 25 ? 2 : 1;
  return `<span class="nt-sig">${[1, 2, 3, 4].map((k) => `<i class="${k <= n ? "on" : ""}" style="height:${k * 25}%"></i>`).join("")}</span>`;
}

// ---------------------------------------------------------------- página

export async function render(el) {
  el.innerHTML =
    head("RESET EXPLODE", "Rede", `<button class="btn" id="test" type="button">Testar</button>`) +
    `<p class="sub nt-lead">Descubra se o lag vem do seu PC, do Wi-Fi ou do provedor.</p>
    <div class="grid wide nt-top">
      <div class="card" id="conn"><div class="sub">Lendo a conexão…</div></div>
      <div class="card"><div class="row"><h2 style="margin:0">Tráfego agora</h2><span class="spacer"></span><span class="faint small" id="trafIf"></span></div>
        <div class="nt-rates"><div><span class="faint">↓ recebendo</span><div class="big" id="rx">—</div></div><div><span class="faint">↑ enviando</span><div class="big" id="tx">—</div></div></div>
        <div class="nt-chart"><canvas id="trafChart" style="height:150px"></canvas><div class="nt-tip" hidden></div></div>
        ${legend(["Recebendo", "Enviando"])}
        <div class="sub" id="trafTotal" style="margin-top:6px"></div></div>
    </div>
    <div class="card nt-test" id="testCard">
      <div class="row"><h2 style="margin:0">Diagnóstico de lag</h2><span class="spacer"></span><span class="sub" id="stage"></span></div>
      <div class="nt-progress"><i id="prog"></i></div>
      <div id="testBody"></div>
    </div>
    <div class="card" style="margin-top:12px"><h2>Adaptadores</h2><div id="ifaces"></div></div>
    <p class="faint small" style="margin-top:14px">O teste envia pacotes de ping ao roteador e aos DNS públicos 1.1.1.1 e 8.8.8.8, consulta o DNS e baixa/envia
      dados de teste do servidor público speed.cloudflare.com (cerca de 100 MB no total). Feche downloads, streaming e jogos antes para um resultado fiel.</p>`;

  const $ = (id) => el.querySelector("#" + id);
  let ov = null;

  // ---- conexão e adaptadores
  async function loadOverview() {
    ov = await call("network", "overview");
    drawConn();
    drawIfaces();
  }

  function drawConn() {
    const i = ov.interfaces.find((x) => x.default);
    if (!i) {
      $("conn").innerHTML = `<h2>Conexão</h2><div class="check"><span class="dot bad"></span><div><b>Sem conexão</b>
        <div class="sub">Nenhum adaptador conectado com acesso ao roteador. Verifique o cabo, o Wi-Fi ou o adaptador.</div></div></div>`;
      return;
    }
    const w = i.wifi;
    const title = w ? w.ssid ?? "Wi-Fi" : i.kind === "ethernet" ? "Cabo" : i.kind_label;
    const link = w
      ? [w.band, w.standard, w.tx_mbps && `${num(w.tx_mbps)} Mbps`].filter(Boolean).join(" · ")
      : [i.speed_mbps && `${num(i.speed_mbps)} Mbps`, i.eth?.duplex && `duplex ${i.eth.duplex}`].filter(Boolean).join(" · ");
    const v6 = i.ipv6.find((a) => !a.startsWith("fe80")) ?? i.ipv6[0];
    const row = (k, v) => (v ? `<dt>${esc(k)}</dt><dd>${v}</dd>` : "");
    $("conn").innerHTML = `<div class="row"><h2 style="margin:0">Conexão ativa</h2><span class="spacer"></span><span class="tag">${esc(i.kind_label)}</span></div>
      <div class="nt-conn"><span class="nt-ico">${icon(i.kind)}</span><div style="min-width:0">
        <div class="nt-conn-name ellipsis">${esc(title)}</div><div class="sub ellipsis">${esc(i.model ?? i.driver ?? i.name)}</div></div>
        ${w ? `<div class="nt-signal">${signalBars(w.signal_pct)}<span class="mono">${w.signal_dbm ?? "—"} dBm</span></div>` : ""}</div>
      <dl class="kv">
        ${row("Interface", esc(i.name))}
        ${row("Link", esc(link || "—"))}
        ${row("Endereço IPv4", esc(i.ipv4.join(", ") || "—"))}
        ${row("Endereço IPv6", v6 && esc(v6))}
        ${row("Gateway", esc(ov.gateway?.address ?? "—"))}
        ${row("DNS", esc(ov.dns.join(", ") || "—") + (ov.resolved ? ` <span class="faint">via systemd-resolved</span>` : ""))}
        ${row("MAC", i.mac && esc(i.mac))}
        ${row("Nome do PC", ov.hostname && esc(ov.hostname))}
      </dl>
      ${i.note ? `<div class="test-banner">${esc(i.note)}</div>` : ""}`;
  }

  function drawIfaces() {
    $("ifaces").innerHTML = ov.interfaces.length
      ? ov.interfaces
          .map((i) => {
            const w = i.wifi;
            const state = i.up ? (w ? `${esc(w.ssid ?? "conectado")}` : i.speed_mbps ? `${num(i.speed_mbps)} Mbps` : "conectado") : w ? "desconectado" : "sem cabo / desligado";
            const errs = i.rx_errors + i.tx_errors + i.rx_dropped;
            return `<div class="nt-if"><span class="nt-ico sm">${icon(i.kind)}</span>
              <div class="nt-if-main"><div class="row" style="gap:8px"><b class="mono">${esc(i.name)}</b><span class="tag">${esc(i.kind_label)}</span>${i.default ? `<span class="tag ok">internet</span>` : ""}</div>
                <div class="sub ellipsis">${esc([i.model, i.driver && `driver ${i.driver}`].filter(Boolean).join(" · ") || "—")}</div></div>
              <div class="nt-if-col"><span class="faint">Endereço</span><b>${esc(i.ipv4[0] ?? "—")}</b></div>
              <div class="nt-if-col"><span class="faint">MAC</span><b>${esc(i.mac ?? "—")}</b></div>
              <div class="nt-if-col nt-wide"><span class="faint">Tráfego desde o boot</span><b>↓ ${bytes(i.rx_bytes)} · ↑ ${bytes(i.tx_bytes)}</b></div>
              <div class="nt-if-state"><span class="dot ${i.up ? "ok" : ""}"></span><span>${state}${w && i.up && w.signal_dbm != null ? ` · ${w.signal_dbm} dBm` : ""}</span>
                ${errs ? `<span class="faint small" title="Erros e pacotes descartados desde o boot">${num(errs)} erros/descartes</span>` : ""}</div></div>`;
          })
          .join("")
      : `<div class="empty">Nenhum adaptador de rede encontrado.</div>`;
  }

  // ---- tráfego ao vivo (adaptador da internet)
  const hist = { rx: [], tx: [] };
  let prev = null;
  async function traffic() {
    const t = await call("network", "traffic");
    const name = ov?.interfaces.find((x) => x.default)?.name ?? t[0]?.name;
    const cur = t.find((x) => x.name === name);
    if (!cur) return;
    const now = performance.now();
    if (prev && prev.name === name) {
      const dt = (now - prev.at) / 1000;
      const rx = (8 * Math.max(0, cur.rx - prev.rx)) / dt, tx = (8 * Math.max(0, cur.tx - prev.tx)) / dt;
      hist.rx.push(rx / 1e6);
      hist.tx.push(tx / 1e6);
      if (hist.rx.length > 60) (hist.rx.shift(), hist.tx.shift());
      const [a, au] = rate(rx), [b, bu] = rate(tx);
      $("rx").innerHTML = `${a}<small>${au}</small>`;
      $("tx").innerHTML = `${b}<small>${bu}</small>`;
    }
    prev = { name, at: now, rx: cur.rx, tx: cur.tx };
    $("trafIf").textContent = name;
    $("trafTotal").textContent = `Desde que o PC ligou: ↓ ${bytes(cur.rx)} recebidos · ↑ ${bytes(cur.tx)} enviados`;
    const n = hist.rx.length;
    const pts = (a) => a.map((v, i) => [i - n + 1, v]);
    lineChart($("trafChart"), {
      series: [{ name: "Recebendo", points: pts(hist.rx) }, { name: "Enviando", points: pts(hist.tx) }],
      x0: -59, x1: 0, ticks: [-60, -45, -30, -15, 0].map((x) => Math.max(x, -59)),
      yMin: 0.1, snap: 0.5,
      fmtX: (x, long) => (x === 0 ? "agora" : long ? `há ${-x} s` : `${-x}s`),
      fmtY: (v, axis) => (axis ? (v >= 10 ? num(v) : num(v, 1)) : `${num(v, v >= 100 ? 0 : 2)} Mbps`),
      empty: "medindo…",
    });
  }

  // ---- diagnóstico
  function drawTest(kind) {
    $("stage").textContent = S.stage;
    $("prog").style.width = `${Math.round(S.frac * 100)}%`;
    $("prog").parentElement.classList.toggle("on", S.running);
    $("test").textContent = S.running ? "Cancelar" : S.report ? "Testar de novo" : "Testar";
    $("test").classList.toggle("outline", S.running);

    const body = $("testBody");
    if (!S.running && !S.report && !S.names.length) {
      body.innerHTML = `<div class="nt-intro">
        <div><b>Latência, jitter e perda</b><span>30 pings ao roteador, à Cloudflare e ao Google, em paralelo.</span></div>
        <div><b>DNS</b><span>Quanto o seu DNS demora para responder, comparado a 1.1.1.1 e 8.8.8.8.</span></div>
        <div><b>Velocidade</b><span>Download e upload, com a latência medida durante o download (bufferbloat).</span></div></div>`;
      return;
    }
    // estrutura uma vez; depois só repinta o que mudou
    if (!body.querySelector("#latChart")) {
      body.innerHTML = `<div id="verdict"></div>
        <div class="nt-grid">
          <div><label class="field">Latência por pacote</label>
            <div class="nt-chart"><canvas id="latChart" style="height:170px"></canvas><div class="nt-tip" hidden></div></div>
            <div id="latLegend"></div><div id="latTable"></div></div>
          <div><label class="field">Velocidade</label>
            <div class="nt-speed"><div><span class="faint">↓ download</span><div class="big" id="down">—</div></div><div><span class="faint">↑ upload</span><div class="big" id="up">—</div></div></div>
            <div class="nt-chart"><canvas id="spdChart" style="height:110px"></canvas><div class="nt-tip" hidden></div></div>
            ${legend(["Download", "Upload"])}
            <div class="stat-row" id="bloat"></div></div>
        </div>
        <label class="field">DNS (tempo de resposta)</label><div id="dns"></div>`;
    }
    if (kind === "stage" && body.querySelector("#latTable").innerHTML) return;
    drawVerdict();
    drawLatency();
    drawSpeed();
    drawDns();
  }

  function drawVerdict() {
    const r = S.report;
    if (!r) {
      $("verdict").innerHTML = "";
      return;
    }
    const dot = { good: "ok", warn: "warn", bad: "bad" }[r.verdict];
    $("verdict").innerHTML = `<div class="nt-verdict"><span class="dot ${dot}"></span><div style="min-width:0"><div class="row" style="gap:10px"><b class="nt-label">${esc(r.label)}</b>
        <span class="sub">${esc(r.adapter ?? "")}${S.at ? ` · testado às ${S.at.toLocaleTimeString("pt-BR")}` : ""}</span></div>
        <ul class="nt-reasons">${r.reasons.map((x) => `<li>${esc(x)}</li>`).join("")}</ul></div></div>`;
  }

  function drawLatency() {
    const names = S.names.map((n) => n.replace("Internet · ", ""));
    $("latLegend").innerHTML = names.length ? legend(names) : "";
    lineChart($("latChart"), {
      series: names.map((n, i) => ({ name: n, points: S.pings[i] })),
      x0: 1, x1: 30, ticks: [1, 10, 20, 30],
      fmtX: (x, long) => (long ? `pacote ${x}` : `${x}`),
      fmtY: (v, axis) => (axis ? num(v) : ms(v)),
      empty: "aguardando os pings…",
    });
    const stats = S.report?.pings;
    const rows = names.map((n, i) => {
      const st = stats?.[i];
      const pts = S.pings[i].map((p) => p[1]);
      const avg = st ? st.avg_ms : pts.length ? pts.reduce((a, b) => a + b, 0) / pts.length : null;
      const jit = st ? st.jitter_ms : pts.length > 1 ? pts.slice(1).reduce((a, v, k) => a + Math.abs(v - pts[k]), 0) / (pts.length - 1) : null;
      const loss = st ? (st.answered ? `${num(st.loss_pct, st.loss_pct % 1 ? 1 : 0)}%` : "não responde") : "…";
      return `<tr><td>${legendKey(i)} ${esc(n)} <span class="faint mono">${esc(S.addresses[i] ?? "")}</span></td>
        <td class="num">${ms(avg)}</td><td class="num muted">${ms(st?.answered ? st.min_ms : pts.length ? Math.min(...pts) : null)}</td>
        <td class="num muted">${ms(st?.answered ? st.max_ms : pts.length ? Math.max(...pts) : null)}</td><td class="num">${ms(jit)}</td><td class="num">${loss}</td></tr>`;
    });
    $("latTable").innerHTML = names.length
      ? `<table class="nt-table"><thead><tr><th>DESTINO</th><th class="num">MÉDIA</th><th class="num">MÍN.</th><th class="num">MÁX.</th><th class="num">JITTER</th><th class="num">PERDA</th></tr></thead><tbody>${rows.join("")}</tbody></table>`
      : "";
  }

  function drawSpeed() {
    const r = S.report;
    const last = (a) => a[a.length - 1]?.[2];
    const down = r ? r.download_mbps : last(S.speed.down);
    const up = r ? r.upload_mbps : last(S.speed.up);
    const big = (v) => (v == null ? "—" : `${num(v, v >= 100 ? 0 : 1)}<small>Mbps</small>`);
    $("down").innerHTML = big(down);
    $("up").innerHTML = big(up);
    const toPts = (a) => smooth(a.map((p) => [Math.round(p[0] * 4) / 4, p[1]]));
    lineChart($("spdChart"), {
      series: [{ name: "Download", points: toPts(S.speed.down) }, { name: "Upload", points: toPts(S.speed.up) }],
      x0: 0, x1: 8, ticks: [0, 2, 4, 6, 8], snap: 0.2,
      fmtX: (x, long) => (long ? `${num(x, 2)} s` : `${x}s`),
      fmtY: (v, axis) => (axis ? num(v) : mbps(v)),
      empty: r?.speed_error ? "não medido" : "aguardando…",
    });
    // bufferbloat: latência durante o download contra a melhor latência ociosa até a internet
    const loaded = r ? r.loaded_ms : S.loaded.length ? S.loaded.reduce((a, p) => a + p[1], 0) / S.loaded.length : null;
    const idle = (r?.pings ?? []).slice(1).filter((p) => p.answered).map((p) => p.avg_ms);
    const base = idle.length ? Math.min(...idle) : null;
    $("bloat").innerHTML = `<div>Latência ociosa<b>${ms(base)}</b></div><div>Durante o download<b>${ms(loaded)}</b></div>
      <div>Aumento<b>${loaded != null && base != null ? `+${ms(Math.max(0, loaded - base), 0)}` : "—"}</b></div>`;
  }

  function drawDns() {
    const list = S.report?.dns ?? S.dns;
    if (!list.length) {
      $("dns").innerHTML = `<div class="sub">${S.running ? "Aguardando…" : "—"}</div>`;
      return;
    }
    const max = Math.max(...list.map((d) => d.ms ?? 0), 1);
    const best = Math.min(...list.filter((d) => d.ms != null).map((d) => d.ms));
    $("dns").innerHTML = `<div class="nt-dns">${list
      .map((d) => `<div class="nt-dns-row"><span class="nt-dns-name">${esc(d.name)} <span class="faint mono">${esc(d.server)}</span>${d.yours ? ` <span class="tag">o seu</span>` : ""}</span>
        <div class="bar"><i style="width:${d.ms == null ? 0 : Math.max(2, (100 * d.ms) / max)}%"></i></div>
        <b class="mono">${d.ms == null ? "não respondeu" : ms(d.ms, 0)}${d.ms === best && list.length > 1 ? ` <span class="faint">· mais rápido</span>` : ""}</b></div>`)
      .join("")}</div>`;
  }

  // ---- ligações
  if (!S.listening) {
    S.listening = true;
    await listen("network-progress", onEvent);
    // o app pode ter sido recarregado no meio de um teste
    const st = await call("network", "status");
    S.running = st.running;
    if (st.running) S.stage = "Teste em andamento…";
    if (!S.report && st.last) S.report = st.last;
  }
  let frame = 0;
  S.redraw = (kind) => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => drawTest(kind));
  };
  $("test").addEventListener("click", async () => {
    if (S.running) {
      await call("network", "cancel");
      S.stage = "Cancelando…";
      drawTest();
      return;
    }
    reset();
    drawTest();
    try {
      await call("network", "start");
    } catch (e) {
      S.running = false;
      S.stage = `Não começou: ${e}`;
      drawTest();
    }
  });
  drawTest();

  await loadOverview().catch((e) => ($("conn").innerHTML = `<div class="sub">Não deu para ler a rede: ${esc(String(e))}</div>`));
  const stopTraffic = every(1000, traffic);
  // adaptadores mudam pouco (cabo, Wi-Fi): relê a cada 15 s, fora do teste
  const timer = setInterval(() => !S.running && loadOverview().catch(() => {}), 15000);
  const onResize = () => S.redraw?.();
  window.addEventListener("resize", onResize);
  return () => {
    stopTraffic();
    clearInterval(timer);
    window.removeEventListener("resize", onResize);
    cancelAnimationFrame(frame);
    S.redraw = null;
  };
}
