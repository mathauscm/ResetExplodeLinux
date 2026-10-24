// Otimização: os equivalentes Linux dos "Ajustes do Windows para jogos" (estado atual, aplicar, desfazer e
// risco de cada um) e as limpezas (cache do pacman, órfãos, registro, caches do usuário, shaders da Steam).
// No CachyOS boa parte já vem ajustada: aparece como "já ajustado pelo CachyOS" e o app não mexe.
// O que precisa de root roda como tarefa (pkexec) com a saída ao vivo no painel.

import { call, head, esc, bytes, bar } from "../lib.js";
import { runJob } from "../pkgjob.js";

const STATE = {
  applied: ["aplicado pelo app", "ok"],
  ok: ["já está bom", "ok"],
  cachyos: ["já ajustado pelo CachyOS", "ok"],
  off: ["não aplicado", "off"],
  na: ["não disponível", "na"],
};
const GROUPS = ["Processador", "Jogos", "Memória", "Disco", "Kernel"];

const risk = (r) => `<span class="opt-risk r-${r === "baixo" ? "low" : r === "médio" ? "mid" : "high"}" title="Risco: o quanto pode atrapalhar se não combinar com o seu uso">risco ${esc(r)}</span>`;

export async function render(el) {
  el.innerHTML = head("RESET FERRAMENTAS", "Otimização", `<div class="chips" id="tabs">
      <button class="chip on" data-tab="tweaks" type="button">Ajustes</button>
      <button class="chip" data-tab="clean" type="button">Limpeza</button></div>
      <button class="btn outline" id="reload" type="button">Atualizar</button>`) + `
    <div id="hero"></div>
    <div class="card job" id="job" hidden style="margin-bottom:12px"></div>
    <div class="opt-msg" id="msg" hidden></div>
    <div id="body"><div class="empty">Lendo o estado do sistema…</div></div>`;

  const $ = (id) => el.querySelector("#" + id);
  let tab = "tweaks";
  let profile = null;
  let tweaks = null;
  let cleanups = null;
  let confirming = null; // ação esperando o segundo clique
  let busy = false;

  const say = (text, ok = true) => {
    $("msg").hidden = !text;
    $("msg").className = "opt-msg" + (ok ? "" : " bad");
    $("msg").textContent = text ?? "";
  };

  // ---------------- topo: o sistema e o resumo ----------------
  function drawHero() {
    if (!profile || !tweaks) return;
    const count = (s) => tweaks.filter((t) => t.state === s).length;
    const done = count("applied") + count("ok") + count("cachyos");
    const usable = tweaks.filter((t) => t.state !== "na").length;
    const pct = usable ? (100 * done) / usable : 0;
    $("hero").innerHTML = `<div class="card opt-hero">
      <div class="opt-hero-main">
        <div class="eyebrow">${profile.cachyos ? "CachyOS detectado" : "Sistema"}</div>
        <div class="opt-hero-title">${esc(profile.distro)} <span class="faint mono">${esc(profile.kernel)}</span></div>
        <p class="sub">${profile.cachyos
          ? `O CachyOS já vem ajustado para jogos (pacote <code>cachyos-settings</code> ${esc(profile.cachyos_settings)}${profile.cachyos_kernel ? " e kernel próprio" : ""}). O que ele já cuida aparece marcado e o app não mexe.`
          : "Cada ajuste mostra o estado atual, o que muda e o risco. Tudo que o app aplica dá para desfazer."}</p>
      </div>
      <div class="opt-score">
        <div class="big">${done}<small>/ ${usable}</small></div>
        <div class="sub">ajustes em dia</div>
        ${bar(pct)}
        <div class="opt-legend">
          ${count("cachyos") ? `<span><i class="dot ok"></i>${count("cachyos")} pelo CachyOS</span>` : ""}
          ${count("applied") ? `<span><i class="dot ok"></i>${count("applied")} pelo app</span>` : ""}
          ${count("ok") ? `<span><i class="dot ok"></i>${count("ok")} já estavam</span>` : ""}
          ${count("off") ? `<span><i class="dot"></i>${count("off")} para aplicar</span>` : ""}
        </div>
      </div></div>`;
  }

  // ---------------- ajustes ----------------
  function tweakRow(t) {
    const [label, cls] = STATE[t.state] ?? [t.state, "off"];
    const key = (apply) => `tweak:${t.id}:${apply ? 1 : 0}`;
    const btn = (apply, text, primary) => {
      const armed = confirming === key(apply);
      const k = armed ? (apply ? "btn" : "btn danger") : primary ? "btn" : "btn outline";
      return `<button class="${k}" type="button" data-tweak="${esc(t.id)}" data-apply="${apply ? 1 : 0}" ${busy ? "disabled" : ""}>${armed ? "Confirmar" : esc(text)}</button>`;
    };
    return `<div class="opt-row st-${cls}">
      <div class="opt-main">
        <div class="row opt-title"><b>${esc(t.title)}</b><span class="opt-tag t-${cls}${t.state === "cachyos" ? " cachy" : ""}">${esc(label)}</span>${t.restart ? `<span class="tag">vale depois de reiniciar</span>` : ""}</div>
        <div class="sub">${esc(t.what)}</div>
        <div class="opt-effect">${esc(t.effect)}</div>
        <div class="opt-now"><span>Agora</span><code>${esc(t.current || "—")}</code>${t.target && t.state === "off" ? `<span>→</span><code>${esc(t.target)}</code>` : ""}</div>
        ${t.note ? `<div class="opt-note">${esc(t.note)}</div>` : ""}
      </div>
      <div class="opt-side">
        ${risk(t.risk)}
        <div class="opt-actions">
          ${t.can_apply ? btn(true, t.apply_label, true) : ""}
          ${t.can_undo ? btn(false, t.undo_label, false) : ""}
        </div>
        ${(t.can_apply || t.can_undo) && t.root ? `<div class="faint small">pede a senha</div>` : ""}
      </div>
    </div>`;
  }

  function drawTweaks() {
    const groups = GROUPS.filter((g) => tweaks.some((t) => t.group === g));
    $("body").innerHTML = groups
      .map((g) => `<h2 class="opt-group">${esc(g)}</h2><div class="card opt-list">${tweaks.filter((t) => t.group === g).map(tweakRow).join("")}</div>`)
      .join("") + `<p class="sub opt-foot">Os parâmetros do kernel ficam em <code>/etc/sysctl.d/99-reset-explode.conf</code>, com o valor de antes anotado, e "Desfazer" volta exatamente a ele.</p>`;
  }

  async function loadTweaks() {
    [profile, tweaks] = await Promise.all([profile ?? call("tools", "profile"), call("tools", "tweaks")]);
    drawHero();
    if (tab === "tweaks") drawTweaks();
  }

  // ---------------- limpeza ----------------
  function cleanBtn(c, itemKey, text) {
    const k = `clean:${c.id}:${itemKey ?? ""}`;
    const armed = confirming === k;
    return `<button class="btn ${armed ? "danger" : itemKey ? "outline small-btn" : ""}" type="button" data-clean="${esc(c.id)}" ${itemKey != null ? `data-key="${esc(itemKey)}"` : ""} ${busy ? "disabled" : ""}>${armed ? "Confirmar" : esc(text)}</button>`;
  }

  function cleanCard(c) {
    const [label, cls] = c.state === "todo" ? ["dá para limpar", "off"] : STATE[c.state] ?? ["nada a fazer", "ok"];
    const max = Math.max(1, ...c.items.map((i) => i.size));
    const items = c.items.length
      ? c.id === "orphans"
        ? `<div class="opt-pkgs">${c.items.map((i) => `<code>${esc(i.name)}</code>`).join("")}</div>`
        : `<div class="opt-items">${c.items
            .map((i) => `<div class="opt-item">
              <div class="opt-item-main"><div class="row"><span class="ellipsis"><b>${esc(i.name)}</b></span><span class="spacer"></span><span class="mono">${bytes(i.size)}</span></div>
                ${bar((100 * i.size) / max)}
                ${i.detail ? `<div class="faint small ellipsis${/feche/.test(i.detail) ? " warn-text" : ""}">${esc(i.detail)}</div>` : ""}</div>
              ${c.id === "cache" || c.id === "shaders" ? cleanBtn(c, i.key, "Limpar") : ""}</div>`)
            .join("")}</div>`
      : "";
    const canAll = c.all && c.state !== "na" && c.state !== "ok" && (c.reclaim == null || c.reclaim > 0 || c.id === "orphans");
    return `<div class="card opt-clean st-${cls}">
      <div class="row"><h2 style="margin:0">${esc(c.title)}</h2><span class="spacer"></span>${risk(c.risk)}</div>
      <div class="opt-size"><span class="big">${c.id === "orphans" ? c.items.length : bytes(c.size)}</span>
        <span class="sub">${c.id === "orphans" ? (c.items.length === 1 ? "pacote" : "pacotes") : "ocupados"}${c.reclaim > 0 && c.id !== "orphans" ? ` · libera <b class="fg">${bytes(c.reclaim)}</b>` : ""}</span></div>
      <div class="sub">${esc(c.what)}</div>
      ${c.note ? `<div class="opt-note">${esc(c.note)}</div>` : ""}
      ${items}
      <div class="row opt-clean-foot"><span class="opt-tag t-${cls}${c.state === "cachyos" ? " cachy" : ""}">${esc(label)}</span><span class="spacer"></span>
        ${c.root && canAll ? `<span class="faint small">pede a senha</span>` : ""}
        ${canAll ? cleanBtn(c, null, c.id === "shaders" ? "Limpar todos" : c.action) : ""}</div>
    </div>`;
  }

  function drawClean() {
    const total = cleanups.reduce((s, c) => s + (c.reclaim ?? (c.id === "cache" ? c.size : 0)), 0);
    $("body").innerHTML = `<p class="sub opt-intro">Dá para liberar até <b class="fg">${bytes(total)}</b>. Nada é apagado sem confirmar: o primeiro clique arma, o segundo confirma.</p>
      <div class="grid wide opt-clean-grid">${cleanups.map(cleanCard).join("")}</div>`;
  }

  async function loadClean() {
    $("body").innerHTML = `<div class="empty">Medindo caches e pacotes…</div>`;
    cleanups = await call("tools", "cleanups");
    if (tab === "clean") drawClean();
  }

  const redraw = () => (tab === "tweaks" ? tweaks && drawTweaks() : cleanups && drawClean());

  // ---------------- eventos ----------------
  $("tabs").addEventListener("click", (e) => {
    const b = e.target.closest("[data-tab]");
    if (!b) return;
    tab = b.dataset.tab;
    $("hero").hidden = tab !== "tweaks";
    confirming = null;
    el.querySelectorAll("#tabs .chip").forEach((c) => c.classList.toggle("on", c === b));
    if (tab === "tweaks") tweaks ? drawTweaks() : loadTweaks();
    else cleanups ? drawClean() : loadClean();
  });

  $("reload").addEventListener("click", () => {
    confirming = null;
    say(null);
    if (tab === "tweaks") loadTweaks();
    else loadClean();
  });

  $("body").addEventListener("click", async (e) => {
    const tw = e.target.closest("[data-tweak]");
    const cl = e.target.closest("[data-clean]");
    if (!tw && !cl) return;
    const k = tw ? `tweak:${tw.dataset.tweak}:${tw.dataset.apply}` : `clean:${cl.dataset.clean}:${cl.dataset.key ?? ""}`;
    if (confirming !== k) {
      // primeiro clique só arma (sem diálogo do navegador, que o Tauri bloqueia)
      confirming = k;
      redraw();
      return;
    }
    confirming = null;
    busy = true;
    say(null);
    redraw();
    try {
      if (tw) {
        const t = tweaks.find((x) => x.id === tw.dataset.tweak);
        const apply = tw.dataset.apply === "1";
        await runJob(() => call("tools", "tweak", { id: t.id, apply }), $("job"), `${apply ? t.apply_label : t.undo_label}: ${t.title}`);
        busy = false;
        await loadTweaks();
      } else {
        const c = cleanups.find((x) => x.id === cl.dataset.clean);
        const key = cl.dataset.key ?? null;
        if (c.root) {
          await runJob(() => call("tools", "clean", { id: c.id }), $("job"), c.title);
        } else {
          const r = await call("tools", "clean", { id: c.id, key });
          const what = key ? c.items.find((i) => i.key === key)?.name ?? key : c.title;
          say(`${what}: ${bytes(r.freed)} liberados.`);
        }
        busy = false;
        await loadClean();
      }
    } catch (err) {
      busy = false;
      say(String(err), false);
      redraw();
    }
  });

  await loadTweaks();
}
