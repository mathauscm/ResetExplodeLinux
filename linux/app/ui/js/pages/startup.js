// Inicialização: o que abre junto com o sistema (equivalente à tela do Windows que usava as chaves Run e a
// pasta Inicializar). Apps ao entrar (autostart), serviços do usuário e do sistema, cada um com o impacto
// (quanto atrasou o boot, memória e CPU), desativar/reativar e, para os atalhos do usuário, remover com
// "restaurar". No topo, o tempo de boot e o que mais pesou nele.

import { call, head, esc, num, bytes } from "../lib.js";
import { runJob } from "../pkgjob.js";

const TABS = [
  ["autostart", "Apps ao entrar"],
  ["user", "Serviços do usuário"],
  ["system", "Serviços do sistema"],
];
const PHASES = { firmware: "Placa-mãe (BIOS)", loader: "Carregador de boot", kernel: "Kernel", initrd: "initramfs", userspace: "Sistema e serviços" };
const GEAR = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>`;

const secs = (ms) => (ms == null ? "—" : ms >= 1000 ? `${num(ms / 1000, 1)} s` : `${num(ms)} ms`);
const avatar = (e) =>
  e.icon ? `<img class="app-icon st-icon" src="${e.icon}" alt="">` : e.kind === "autostart" ? `<div class="app-icon st-icon letter">${esc(e.name[0] ?? "?")}</div>` : `<div class="app-icon st-icon gear">${GEAR}</div>`;

export async function render(el) {
  el.innerHTML = head("RESET FERRAMENTAS", "Inicialização", `<button class="btn outline" id="reload" type="button">Atualizar</button>`) + `
    <div class="grid wide st-top">
      <div class="card" id="boot"><h2>Tempo de boot</h2><div class="sub">Lendo o systemd-analyze…</div></div>
      <div class="card" id="slow"><h2>O que mais pesou no boot</h2><div class="sub">Lendo…</div></div>
    </div>
    <div class="card job" id="job" hidden style="margin-top:12px"></div>
    <div class="opt-msg st-msg" id="msg" hidden></div>
    <div class="row st-bar"><div class="chips" id="tabs"></div><span class="spacer"></span><span class="sub" id="summary"></span></div>
    <div id="body"><div class="empty">Lendo o que abre com o sistema…</div></div>
    <div id="removed"></div>`;

  const $ = (id) => el.querySelector("#" + id);
  let tab = "autostart";
  let data = null;
  let confirming = null;
  let busy = false;

  const say = (text, ok = true) => {
    $("msg").hidden = !text;
    $("msg").className = "opt-msg st-msg" + (ok ? "" : " bad");
    $("msg").textContent = text ?? "";
  };

  // ---------------- tempo de boot ----------------
  async function loadBoot() {
    const b = await call("tools", "boot");
    const t = b.times;
    if (!t) {
      $("boot").innerHTML = `<h2>Tempo de boot</h2><div class="sub">O systemd ainda não terminou de contar o boot (ou não deu para ler).</div>`;
    } else {
      const parts = t.parts.filter(([, ms]) => ms > 0);
      $("boot").innerHTML = `<h2>Tempo de boot</h2>
        <div class="big">${num(t.total_ms / 1000, 1)}<small>s</small></div>
        <div class="st-phases">${parts.map(([n, ms], i) => `<i class="ph-${i}" style="flex:${ms}" title="${esc(PHASES[n] ?? n)}: ${secs(ms)}"></i>`).join("")}</div>
        <div class="st-legend">${parts.map(([n, ms], i) => `<div><i class="ph-${i}"></i><span>${esc(PHASES[n] ?? n)}</span><b>${secs(ms)}</b></div>`).join("")}</div>
        <div class="st-after">${t.target_ms != null ? `<span>Tela de login <b>${secs(t.target_ms)}</b> depois do kernel</span>` : ""}${b.user_ms != null ? `<span>Sua sessão <b>${secs(b.user_ms)}</b></span>` : ""}</div>
        ${b.tips.map((tip) => `<div class="test-banner st-tip">${esc(tip)}</div>`).join("")}`;
    }
    const max = Math.max(1, ...b.slow.map((s) => s.ms));
    const rows = (scope) => b.slow.filter((s) => s.scope === scope).map((s) => `<div class="st-slow">
        <div class="row"><span class="mono ellipsis st-unit">${esc(s.unit.replace(/\.service$/, ""))}</span><span class="spacer"></span><b class="mono">${secs(s.ms)}</b></div>
        <div class="st-slowbar"><i style="width:${(100 * s.ms) / max}%"></i></div>
        ${s.hint ? `<div class="faint small">${esc(s.hint)}</div>` : ""}</div>`).join("");
    $("slow").innerHTML = `<h2>O que mais pesou no boot</h2>
      ${rows("system") || `<div class="sub">Sem dados.</div>`}
      ${b.slow.some((s) => s.scope === "user") ? `<div class="st-sub-h">Na sua sessão</div>${rows("user")}` : ""}
      <p class="faint small" style="margin:10px 0 0">Serviços sobem em paralelo: somar os tempos dá mais que o boot inteiro.</p>`;
  }

  // ---------------- lista ----------------
  function impact(e) {
    const i = e.impact;
    const bits = [];
    if (i.boot_ms != null && i.boot_ms > 0) bits.push(`boot ${secs(i.boot_ms)}`);
    if (i.memory != null) bits.push(bytes(i.memory));
    if (i.cpu_ms != null && i.cpu_ms > 0) bits.push(`CPU ${secs(i.cpu_ms)}`);
    if (!i.level) return e.enabled && e.applies ? `<span class="st-imp none">${i.running ? "rodando" : "não está rodando"}</span>` : "";
    const cls = i.level === "alto" ? "hi" : i.level === "médio" ? "mid" : "lo";
    return `<span class="st-imp ${cls}" title="Impacto: quanto atrasou o boot, memória e CPU que usa agora"><i></i>impacto ${esc(i.level)}${bits.length ? ` <span class="faint">· ${esc(bits.join(" · "))}</span>` : ""}</span>`;
  }

  function stateTag(e) {
    if (!e.applies) return `<span class="opt-tag t-na">não roda nesta sessão</span>`;
    if (e.missing) return `<span class="opt-tag st-bad">programa não existe</span>`;
    if (e.enabled) return `<span class="opt-tag t-ok">ativado</span>`;
    return `<span class="opt-tag">${e.by_us ? "desativado pelo app" : "desativado"}</span>`;
  }

  function actions(e) {
    if (e.locked) return `<span class="st-lock" title="${esc(e.locked)}">${esc(e.locked)}</span>`;
    if (!e.applies && e.enabled) return "";
    const k = (a) => `${a}:${e.id}`;
    const armed = (a) => confirming === k(a);
    const toggle = e.enabled
      ? `<button class="btn ${armed("off") ? "danger" : "outline"}" type="button" data-act="off" data-id="${esc(e.id)}" ${busy ? "disabled" : ""}>${armed("off") ? "Confirmar" : "Desativar"}</button>`
      : `<button class="btn" type="button" data-act="on" data-id="${esc(e.id)}" ${busy ? "disabled" : ""}>Reativar</button>`;
    const remove = e.can_remove
      ? `<button class="btn ${armed("rm") ? "danger" : "outline"}" type="button" data-act="rm" data-id="${esc(e.id)}" ${busy ? "disabled" : ""}>${armed("rm") ? "Confirmar remoção" : "Remover"}</button>`
      : "";
    return `<div class="opt-actions">${toggle}${remove}</div>${e.kind === "system" ? `<div class="faint small">pede a senha</div>` : ""}`;
  }

  function row(e) {
    return `<div class="app-row st-row${e.enabled ? "" : " off"}${!e.applies ? " na" : ""}">
      ${avatar(e)}
      <div class="st-main">
        <div class="row st-title"><b>${esc(e.name)}</b>${stateTag(e)}${e.also.length ? `<span class="tag" title="Vai junto ao desativar">+ ${esc(e.also.join(", "))}</span>` : ""}</div>
        ${e.comment ? `<div class="sub ellipsis">${esc(e.comment)}</div>` : ""}
        ${e.hint ? `<div class="st-hint">${esc(e.hint)}</div>` : ""}
        ${e.note ? `<div class="faint small">${esc(e.note)}</div>` : ""}
        <div class="faint mono ellipsis" title="${esc(e.command)}">${esc(e.command || "—")}</div>
        <div class="row st-meta">${impact(e)}${!e.locked && e.risk ? `<span class="opt-risk r-${e.risk === "baixo" ? "low" : e.risk === "médio" ? "mid" : "high"}">risco ${esc(e.risk)}</span>` : ""}<span class="faint small">${esc(e.source)}</span></div>
      </div>
      <div class="st-side">${actions(e)}</div>
    </div>`;
  }

  const order = (a, b) => {
    const w = { alto: 3, "médio": 2, baixo: 1, "": 0 };
    return (b.enabled - a.enabled) || (w[b.impact.level] - w[a.impact.level]) || a.name.localeCompare(b.name, "pt-BR");
  };

  function draw() {
    const list = data.entries.filter((e) => e.kind === tab);
    $("tabs").innerHTML = TABS.map(([k, n]) => {
      const c = data.entries.filter((e) => e.kind === k && e.applies && !e.locked).length;
      return `<button class="chip${k === tab ? " on" : ""}" data-tab="${k}" type="button">${n} <span class="st-count">${c}</span></button>`;
    }).join("");
    const main = list.filter((e) => e.applies && !e.locked).sort(order);
    const locked = list.filter((e) => e.applies && e.locked).sort(order);
    const other = list.filter((e) => !e.applies).sort(order);
    const on = main.filter((e) => e.enabled).length;
    $("summary").textContent = `${main.length} ${main.length === 1 ? "item" : "itens"} · ${on} ativados · ${main.length - on} desativados`;
    const intro = {
      autostart: "Programas que abrem quando você entra na sessão. Desativar não desinstala nada; vale no próximo login.",
      user: "Serviços que rodam em segundo plano na sua sessão. Desativar não pede senha e vale no próximo login.",
      system: "Serviços do sistema inteiro. Desativar pede a senha e vale no próximo boot; os essenciais ficam bloqueados.",
    }[tab];
    $("body").innerHTML = `<p class="sub st-intro">${intro}</p>
      <div class="card st-list">${main.map(row).join("") || `<div class="empty">Nada aqui.</div>`}</div>
      ${locked.length ? `<details class="card st-more"><summary><b>Peças do sistema</b> <span class="faint">(${locked.length}) · não dá para desativar por aqui</span></summary><div>${locked.map(row).join("")}</div></details>` : ""}
      ${other.length ? `<details class="card st-more"><summary><b>Não rodam nesta sessão</b> <span class="faint">(${other.length}) · são de outro desktop ou o ${esc(data.desktop || "desktop")} usa um serviço no lugar</span></summary><div>${other.map(row).join("")}</div></details>` : ""}`;
    $("removed").innerHTML = data.removed.length
      ? `<h2 style="margin-top:22px">Removidos</h2><div class="card st-list">${data.removed
          .map((r) => `<div class="app-row"><div class="app-icon st-icon letter">${esc(r.name[0] ?? "?")}</div><div class="st-main"><b>${esc(r.name)}</b><div class="faint mono ellipsis">${esc(r.original)}</div></div>
            <button class="btn outline" type="button" data-restore="${esc(r.backup)}" ${busy ? "disabled" : ""}>Restaurar</button></div>`)
          .join("")}</div>`
      : "";
  }

  async function load() {
    data = await call("tools", "startup");
    draw();
  }

  async function act(fn) {
    busy = true;
    say(null);
    draw();
    try {
      const r = await fn();
      if (r?.message) say(r.message);
    } catch (err) {
      say(String(err), false);
    }
    busy = false;
    await load();
  }

  // ---------------- eventos ----------------
  $("tabs").addEventListener("click", (e) => {
    const b = e.target.closest("[data-tab]");
    if (!b) return;
    tab = b.dataset.tab;
    confirming = null;
    draw();
  });

  $("reload").addEventListener("click", () => {
    confirming = null;
    say(null);
    load();
    loadBoot();
  });

  el.addEventListener("click", async (e) => {
    const rs = e.target.closest("[data-restore]");
    if (rs) return act(() => call("tools", "startup_restore", { backup: rs.dataset.restore }));
    const b = e.target.closest("[data-act]");
    if (!b) return;
    const { act: a, id } = b.dataset;
    const item = data.entries.find((x) => x.id === id);
    if (a !== "on" && confirming !== `${a}:${id}`) {
      // desativar e remover pedem o segundo clique; reativar (desfazer) é direto
      confirming = `${a}:${id}`;
      draw();
      return;
    }
    confirming = null;
    if (a === "rm") return act(() => call("tools", "startup_remove", { id }));
    const enabled = a === "on";
    if (item.kind === "system") {
      busy = true;
      draw();
      await runJob(() => call("tools", "startup_set", { id, enabled }), $("job"), `${enabled ? "Reativando" : "Desativando"} ${item.name}`);
      busy = false;
      return load();
    }
    return act(() => call("tools", "startup_set", { id, enabled }));
  });

  loadBoot().catch((err) => ($("boot").innerHTML = `<h2>Tempo de boot</h2><div class="sub">${esc(String(err))}</div>`));
  await load();
}
