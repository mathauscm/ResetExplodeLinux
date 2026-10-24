// Portas seriais (equivalente às "Portas COM" do Windows): o que está conectado, quem está usando cada porta,
// a permissão do usuário e um monitor serial simples. A lista se atualiza sozinha ao conectar e desconectar.

import { call, head, esc, every, listen } from "../lib.js";
import { runJob } from "../pkgjob.js";

const BAUDS = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 74880, 115200, 230400, 250000, 460800, 500000, 921600, 1000000, 2000000];
const ENDINGS = [["lf", "LF (\\n)"], ["crlf", "CR+LF"], ["cr", "CR (\\r)"], ["none", "nada"]];
const MAX_OUT = 150_000; // caracteres guardados no monitor

const store = (k, v) => {
  try {
    if (v === undefined) return localStorage.getItem("re.serial." + k);
    localStorage.setItem("re.serial." + k, v);
  } catch {
    return null;
  }
};

const time = () => {
  const d = new Date();
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
};

/** Título amigável: a placa provável, senão o produto, senão o tipo. */
const title = (p) => p.board ?? p.product ?? (p.com ? `${p.com} da placa-mãe` : p.kind);
/** Nome curto para a lista do monitor: conversores (que vão em muitas placas) pelo chip. */
const short = (p) => (["ch340", "ch9102", "cp210x", "ftdi", "pl2303"].includes(p.chip_id) ? p.chip : title(p));

export async function render(el) {
  el.innerHTML = head("RESET MAKER", "Portas seriais", `<span class="tag" id="live">acompanhando</span><button class="btn outline" id="reload" type="button">Atualizar</button>`) + `
    <p class="sub pt-intro">No Linux não existem portas fantasmas nem números COM presos: a porta (<code>/dev/ttyUSB0</code>, <code>/dev/ttyACM0</code>) nasce quando a placa conecta e some quando ela sai. O número pode mudar de uma conexão para outra; para um nome que nunca muda, use o <b class="fg">nome fixo</b> de cada porta (<code>/dev/serial/by-id</code>).</p>
    <div id="alerts"></div>
    <div class="card job" id="job" hidden style="margin-bottom:12px"></div>
    <div class="pt-layout">
      <div id="list"><div class="card"><div class="sub">Procurando portas…</div></div></div>
      <div class="card pt-monitor">
        <div class="row"><h2 style="margin:0">Monitor serial</h2><span class="spacer"></span><span class="tag" id="monState">fechado</span></div>
        <div class="pt-mon-bar">
          <select id="monPort" title="Porta"></select>
          <select id="monBaud" title="Velocidade (baud)">${BAUDS.map((b) => `<option value="${b}">${b.toLocaleString("pt-BR")}</option>`).join("")}</select>
          <button class="btn" id="monOpen" type="button">Abrir</button>
        </div>
        <pre class="pt-out" id="out"></pre>
        <div class="pt-mon-tools">
          <label class="row sub"><input type="checkbox" id="stamp"> hora</label>
          <label class="row sub"><input type="checkbox" id="follow" checked> rolar junto</label>
          <span class="spacer"></span>
          <button class="link" id="resetBoard" type="button" title="Pulso nas linhas DTR/RTS, como a IDE faz antes de enviar">reiniciar placa</button>
          <button class="link" id="clear" type="button">limpar</button>
        </div>
        <form class="pt-send" id="send">
          <input type="text" id="msg" placeholder="Texto para enviar (Enter)" autocomplete="off" spellcheck="false">
          <select id="ending" title="Final de linha">${ENDINGS.map(([k, n]) => `<option value="${k}">${n}</option>`).join("")}</select>
          <button class="btn outline" type="submit" id="sendBtn">Enviar</button>
        </form>
        <div class="status" id="monMsg">Escolha a porta e a velocidade do <code>Serial.begin()</code> do seu código.</div>
        <p class="faint small" style="margin:6px 0 0">Feche o monitor antes de enviar um sketch pela IDE: a porta só atende um programa por vez.</p>
      </div>
    </div>`;

  const $ = (id) => el.querySelector("#" + id);
  let report = null;
  let lastJson = "";
  let confirmKill = null; // "pid:path" esperando o segundo clique
  let mon = { open: false, path: null };
  let out = "";
  let atLineStart = true;
  let paintQueued = false;
  const history = [];
  let histPos = -1;

  $("monBaud").value = store("baud") ?? "115200";
  $("ending").value = store("ending") ?? "lf";
  $("stamp").checked = store("stamp") === "1";

  // ---------------------------------------------------------------- lista de portas
  function portCard(p) {
    const usedByOthers = p.holders.filter((h) => !h.mine);
    const state = !p.access
      ? `<span class="pt-pill bad">sem permissão</span>`
      : usedByOthers.length
        ? `<span class="pt-pill warn">em uso</span>`
        : p.holders.length
          ? `<span class="pt-pill on">no monitor</span>`
          : `<span class="pt-pill ok">livre</span>`;
    const rows = [
      ["Chip", p.chip ?? (p.uart ? `UART ${p.uart}` : null)],
      ["VID:PID", p.vid ? `${p.vid}:${p.pid}` : null],
      ["Fabricante", p.vendor],
      ["Produto", p.product && p.product !== title(p) ? p.product : null],
      ["Nº de série", p.serial],
      ["Driver", p.driver ? `${p.driver}${p.module && p.module !== p.driver ? ` · módulo ${p.module}` : p.module ? "" : " · no kernel"}` : null],
      ["USB", p.usb ? `porta ${p.usb}${p.speed ? ` · ${p.speed}` : ""}` : null],
      ["Permissão", `${p.owner}:${p.group} ${p.mode}`],
    ].filter(([, v]) => v);
    const fixed = p.by_id ? `/dev/serial/by-id/${p.by_id}` : null;
    return `<div class="card pt-port${mon.path === p.path && mon.open ? " active" : ""}">
      <div class="pt-port-head">
        <div style="min-width:0"><div class="pt-dev">${esc(p.path)}${p.com ? ` <span class="faint">· ${esc(p.com)}</span>` : ""}</div>
          <div class="pt-title">${esc(title(p))}</div></div>
        <span class="spacer"></span>${state}
      </div>
      <div class="row" style="gap:6px;margin:2px 0 10px"><span class="tag">${esc(p.kind)}</span>${p.chip_id ? `<span class="tag">${esc(p.chip_id.toUpperCase())}</span>` : ""}</div>
      <dl class="kv">${rows.map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`).join("")}</dl>
      ${fixed ? `<div class="pt-fixed"><span class="faint small">NOME FIXO</span><code title="${esc(fixed)}">${esc(fixed)}</code><button class="link" type="button" data-copy="${esc(fixed)}">copiar</button></div>` : ""}
      ${p.holders.length ? `<div class="pt-holders">${p.holders.map((h) => {
          const key = `${h.pid}:${p.path}`;
          return `<div class="pt-holder"><span class="dot ${h.mine ? "ok" : "warn"}"></span>
            <div style="flex:1;min-width:0"><b>${h.mine ? "RESET EXPLODE (este monitor)" : esc(h.name)}</b> <span class="faint mono">PID ${h.pid}</span>
              ${h.mine ? "" : `<div class="faint mono ellipsis" title="${esc(h.cmd)}">${esc(h.cmd)}</div>`}</div>
            ${h.mine ? "" : `<button class="btn ${confirmKill === key ? "danger" : "outline"} pt-small" type="button" data-kill="${h.pid}" data-path="${esc(p.path)}">${confirmKill === key ? "Confirmar" : "Encerrar"}</button>`}</div>`;
        }).join("")}</div>` : ""}
      <div class="row" style="margin-top:12px">
        <button class="btn outline pt-small" type="button" data-monitor="${esc(p.path)}" ${p.access ? "" : "disabled"}>${mon.open && mon.path === p.path ? "Aberta no monitor" : "Abrir no monitor"}</button>
        ${!p.access ? `<span class="sub">Sem permissão: veja o aviso no topo.</span>` : ""}
      </div>
    </div>`;
  }

  function drawList() {
    const usb = report.ports.filter((p) => p.vid || p.name.startsWith("rfcomm"));
    const board = report.ports.filter((p) => !usb.includes(p));
    const stuck = report.devices.filter((d) => !d.ports.length && d.state !== "ok");
    let html = "";
    if (usb.length) {
      html += `<h2 class="pt-h">Conectadas agora <span class="faint">· ${usb.length}</span></h2><div class="pt-grid">${usb.map(portCard).join("")}</div>`;
    } else {
      html += `<div class="card pt-empty">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M9 2v6M15 2v6M6 8h12v4a6 6 0 0 1-12 0zM12 18v4"/></svg>
        <b>Nenhuma placa conectada</b>
        <div class="sub">Conecte o Arduino, o ESP32 ou o conversor USB-serial: ele aparece aqui sozinho, sem precisar atualizar.</div>
        <div class="faint small">Não apareceu? Teste outro cabo (muitos cabos são só de carga, sem os fios de dados) e veja <b>Drivers seriais</b>.</div>
      </div>`;
    }
    if (stuck.length)
      html += `<div class="card pt-stuck">${stuck.map((d) => `<div class="check"><span class="dot ${d.state === "bad" ? "bad" : "warn"}"></span><div style="flex:1">
          <b>${esc(d.chip ?? d.board ?? d.product ?? "Placa")} conectado, mas sem porta</b> <span class="faint mono">${esc(d.vid)}:${esc(d.pid)}</span>
          <div class="sub">${esc(d.verdict)}. ${esc(d.advice)}</div></div>
          <button class="btn outline pt-small" type="button" data-go="serial">Drivers seriais</button></div>`).join("")}</div>`;
    if (board.length)
      html += `<h2 class="pt-h" style="margin-top:18px">Da placa-mãe</h2><div class="pt-grid">${board.map(portCard).join("")}</div>`;
    $("list").innerHTML = html;
  }

  function drawAlerts() {
    const perm = report.permission;
    const anyDenied = report.ports.some((p) => !p.access);
    let html = "";
    if (!perm.active && (anyDenied || !report.ports.length || !perm.uaccess_rule)) {
      const groups = `<div class="faint small" style="margin-top:6px">Seus grupos agora (<code>id -Gn</code>): ${esc(perm.groups.join(" "))}</div>`;
      if (perm.configured)
        html += `<div class="card pt-alert"><span class="dot warn"></span><div style="flex:1"><b>Falta sair e entrar de novo na sessão</b>
          <div class="sub">Seu usuário já está no grupo <code>${esc(perm.group)}</code>, mas o grupo novo só vale depois de encerrar a sessão (ou reiniciar o PC).</div>${groups}</div></div>`;
      else if (anyDenied || !perm.uaccess_rule)
        html += `<div class="card pt-alert"><span class="dot ${anyDenied ? "bad" : "warn"}"></span><div style="flex:1">
          <b>Seu usuário não está no grupo <code>${esc(perm.group)}</code></b>
          <div class="sub">As portas seriais pertencem ao grupo <code>${esc(perm.group)}</code>${perm.group === "uucp" ? " (no Arch; em outras distribuições é o dialout)" : ""}. Sem ele, a IDE e o monitor dão <i>Permission denied</i> ao abrir a porta.
          ${perm.uaccess_rule ? "" : " Outra saída: instalar a regra udev em <b>Drivers seriais</b>, que vale na hora para quem está logado."}</div>${groups}</div>
          <div class="pt-alert-act"><button class="btn" id="fixGroup" type="button">Entrar no grupo ${esc(perm.group)}</button><div class="faint small">Pede a senha. Vale depois de sair e entrar.</div></div></div>`;
    }
    const acm = report.ports.some((p) => p.name.startsWith("ttyACM"));
    if (report.modemmanager && acm && !perm.uaccess_rule)
      html += `<div class="card pt-alert"><span class="dot warn"></span><div style="flex:1"><b>O ModemManager está rodando</b>
        <div class="sub">Ele testa placas <code>ttyACM</code> (Arduino Uno/Leonardo, ESP32-S3, Pico) como se fossem modem nos primeiros segundos, e o upload pode falhar. A regra udev em <b>Drivers seriais</b> faz ele ignorar as placas.</div></div>
        <div class="pt-alert-act"><button class="btn outline" type="button" data-go="serial">Drivers seriais</button></div></div>`;
    $("alerts").innerHTML = html;
  }

  async function load() {
    try {
      const r = await call("maker", "ports");
      const j = JSON.stringify(r);
      if (j === lastJson) return;
      lastJson = j;
      report = r;
      drawAlerts();
      drawList();
      fillPortSelect();
      $("live").textContent = "acompanhando";
    } catch (e) {
      $("live").textContent = "erro";
      $("list").innerHTML = `<div class="card"><div class="check"><span class="dot bad"></span><div><b>Não deu para ler as portas</b><div class="sub">${esc(String(e))}</div></div></div></div>`;
    }
  }

  // ---------------------------------------------------------------- monitor serial
  function fillPortSelect() {
    const sel = $("monPort");
    const keep = mon.open ? mon.path : sel.value || store("port");
    const ports = report?.ports ?? [];
    sel.innerHTML = ports.length
      ? ports.map((p) => `<option value="${esc(p.path)}">${esc(p.path)} · ${esc(short(p))}</option>`).join("")
      : `<option value="">nenhuma porta</option>`;
    const first = ports.find((p) => p.vid) ?? ports[0];
    sel.value = ports.some((p) => p.path === keep) ? keep : first?.path ?? "";
    paintMon();
  }

  function paintMon() {
    const has = !!$("monPort").value;
    $("monState").textContent = mon.open ? `aberto · ${Number(mon.baud).toLocaleString("pt-BR")} baud` : "fechado";
    $("monState").classList.toggle("ok", mon.open);
    $("monOpen").textContent = mon.open ? "Fechar" : "Abrir";
    $("monOpen").disabled = !mon.open && !has;
    $("monOpen").classList.toggle("outline", mon.open);
    $("monPort").disabled = mon.open;
    $("monBaud").disabled = mon.open;
    $("msg").disabled = !mon.open;
    $("sendBtn").disabled = !mon.open;
    $("resetBoard").disabled = !mon.open;
  }

  function paintOut() {
    if (paintQueued) return;
    paintQueued = true;
    requestAnimationFrame(() => {
      paintQueued = false;
      const o = $("out");
      if (!o) return;
      o.textContent = out;
      if ($("follow").checked) o.scrollTop = o.scrollHeight;
    });
  }

  function append(text) {
    if ($("stamp").checked) {
      let s = "";
      for (const ch of text.replace(/\r\n/g, "\n")) {
        if (atLineStart && ch !== "\r") s += `[${time()}] `;
        s += ch;
        atLineStart = ch === "\n";
      }
      text = s;
    } else if (text.length) {
      atLineStart = text.endsWith("\n");
    }
    out += text;
    if (out.length > MAX_OUT) out = out.slice(out.length - MAX_OUT * 0.8);
    paintOut();
  }

  function note(text) {
    if (!atLineStart) append("\n");
    out += `— ${text} —\n`;
    atLineStart = true;
    paintOut();
  }

  async function openMon() {
    const path = $("monPort").value;
    const baud = Number($("monBaud").value);
    $("monOpen").disabled = true;
    try {
      const r = await call("maker", "serial_open", { path, baud });
      mon = { open: true, path: r.path, baud: r.baud };
      store("port", path);
      note(`${path} aberta a ${baud.toLocaleString("pt-BR")} baud`);
      $("monMsg").textContent = "Lendo. Placas Arduino reiniciam ao abrir a porta: o começo do programa aparece de novo.";
      $("msg").focus();
    } catch (e) {
      $("monMsg").textContent = String(e);
    }
    paintMon();
    lastJson = "";
    load();
  }

  async function closeMon() {
    await call("maker", "serial_close").catch(() => {});
    if (mon.open) note("porta fechada");
    mon = { open: false, path: null };
    $("monMsg").textContent = "Fechado. A porta está livre para a IDE.";
    paintMon();
    lastJson = "";
    load();
  }

  $("monOpen").addEventListener("click", () => (mon.open ? closeMon() : openMon()));
  $("monBaud").addEventListener("change", (e) => store("baud", e.target.value));
  $("ending").addEventListener("change", (e) => store("ending", e.target.value));
  $("stamp").addEventListener("change", (e) => store("stamp", e.target.checked ? "1" : "0"));
  $("clear").addEventListener("click", () => {
    out = "";
    atLineStart = true;
    paintOut();
  });
  $("resetBoard").addEventListener("click", async () => {
    try {
      await call("maker", "serial_reset");
      note("placa reiniciada (DTR/RTS)");
    } catch (e) {
      $("monMsg").textContent = String(e);
    }
  });
  $("send").addEventListener("submit", async (e) => {
    e.preventDefault();
    const text = $("msg").value;
    if (!mon.open) return;
    try {
      await call("maker", "serial_write", { text, ending: $("ending").value });
      if (text && history[history.length - 1] !== text) history.push(text);
      histPos = -1;
      $("msg").value = "";
      $("monMsg").textContent = text ? `Enviado: ${text}` : "Enviado: final de linha";
    } catch (err) {
      $("monMsg").textContent = String(err);
    }
  });
  // setas para cima/baixo repetem o que já foi enviado
  $("msg").addEventListener("keydown", (e) => {
    if (!history.length || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    e.preventDefault();
    histPos = histPos < 0 ? history.length : histPos;
    histPos = Math.max(0, Math.min(history.length, histPos + (e.key === "ArrowUp" ? -1 : 1)));
    $("msg").value = history[histPos] ?? "";
  });

  const unData = await listen("maker-serial", (p) => append(p.data));
  const unClosed = await listen("maker-serial-closed", (p) => {
    note(`a porta caiu: ${p.reason}`);
    mon = { open: false, path: null };
    $("monMsg").textContent = "A porta fechou sozinha (placa desconectada?). Conecte de novo e clique em Abrir.";
    paintMon();
  });

  // ---------------------------------------------------------------- ações da lista
  el.addEventListener("click", async (e) => {
    const t = e.target.closest("button");
    if (!t) return;
    if (t.dataset.copy) {
      try {
        await navigator.clipboard.writeText(t.dataset.copy);
        t.textContent = "copiado";
      } catch {
        t.textContent = "não deu para copiar";
      }
      setTimeout(() => (t.textContent = "copiar"), 1500);
    } else if (t.dataset.go) {
      document.querySelector(`.nav-item[data-id="${t.dataset.go}"]`)?.click();
    } else if (t.dataset.monitor) {
      if (mon.open && mon.path === t.dataset.monitor) return;
      if (mon.open) await closeMon();
      $("monPort").value = t.dataset.monitor;
      openMon();
    } else if (t.dataset.kill) {
      const key = `${t.dataset.kill}:${t.dataset.path}`;
      if (confirmKill !== key) {
        confirmKill = key; // o primeiro clique só arma
        drawList();
        return;
      }
      confirmKill = null;
      try {
        await call("maker", "kill_holder", { pid: Number(t.dataset.kill), path: t.dataset.path });
      } catch (err) {
        $("monMsg").textContent = String(err);
      }
      lastJson = "";
      setTimeout(load, 400);
    } else if (t.id === "fixGroup") {
      t.disabled = true;
      const g = report.permission.group;
      const r = await runJob(() => call("maker", "fix_group", { group: g }), $("job"), `Colocando ${report.permission.user} no grupo ${g}`);
      if (r.ok)
        $("job").insertAdjacentHTML("beforeend", `<div class="test-banner">Pronto. <b>Saia da sessão e entre de novo</b> (ou reinicie o PC) para o grupo <code>${esc(g)}</code> valer.</div>`);
      lastJson = "";
      load();
    } else if (t.id === "reload") {
      lastJson = "";
      load();
    }
  });

  try {
    const s = await call("maker", "serial_state");
    if (s.open) mon = { open: true, path: s.path, baud: s.baud };
  } catch {
    /* sem backend: segue fechado */
  }
  paintMon();
  const stop = every(2000, load);
  return () => {
    stop();
    unData();
    unClosed();
    // sair da tela libera a porta: a IDE não fica esperando um monitor esquecido
    if (mon.open) call("maker", "serial_close").catch(() => {});
  };
}
