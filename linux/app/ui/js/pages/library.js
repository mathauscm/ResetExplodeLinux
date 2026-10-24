// Biblioteca: jogos instalados (Steam, Heroic, Lutris) em grade de capas. Cada jogo tem um perfil de inicialização
// (equivalente ao RESET JOGOS do Windows) que vira a linha de "Opções de inicialização" da Steam: o app monta, mostra e
// copia; nada é gravado nos arquivos da Steam.

import { call, head, esc, num, bytes, kv } from "../lib.js";

// capas já carregadas ficam na memória: voltar para a tela não lê tudo de novo
const artCache = new Map();

const WIN_MODES = [["default", "Padrão do jogo"], ["full", "Tela cheia"], ["borderless", "Sem bordas"], ["window", "Janela"]];
const RESES = [["default", "Padrão"], ["1280x720", "1280x720"], ["1920x1080", "1920x1080"], ["2560x1440", "2560x1440"], ["3840x2160", "3840x2160"]];
const HZS = [[0, "Padrão"], [60, "60"], [120, "120"], [144, "144"], [165, "165"], [240, "240"]];
const STYLES = [["source", "Source / CS2"], ["unity", "Unity"], ["unreal", "Unreal"]];
const SORTS = [["name", "Nome"], ["recent", "Jogados recentemente"], ["size", "Tamanho"]];

function ago(t) {
  if (!t) return "nunca";
  const days = Math.floor((Date.now() / 1000 - t) / 86400);
  if (days <= 0) return "hoje";
  if (days === 1) return "ontem";
  if (days < 30) return `há ${days} dias`;
  return new Date(t * 1000).toLocaleDateString("pt-BR");
}

const playtime = (m) => (m == null ? null : m < 60 ? `${m} min` : `${num(m / 60, m < 600 ? 1 : 0)} h`);

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // fallback para webviews sem a API de área de transferência
    const t = document.createElement("textarea");
    t.value = text;
    document.body.appendChild(t);
    t.select();
    const ok = document.execCommand("copy");
    t.remove();
    return ok;
  }
}

const chips = (name, list, cur) =>
  `<div class="chips">${list.map(([v, l]) => `<button class="chip${String(v) === String(cur) ? " on" : ""}" type="button" data-set="${name}" data-v="${esc(v)}">${esc(l)}</button>`).join("")}</div>`;

export async function render(el) {
  el.innerHTML = head("RESET JOGOS", "Biblioteca", `<input type="text" id="search" placeholder="Buscar jogo" class="lib-search">
      <button class="btn outline" id="reload" type="button">Atualizar</button>`) + `<div id="body"><div class="empty">Procurando seus jogos…</div></div>`;
  const $ = (id) => el.querySelector("#" + id);

  let data = null;
  let filter = "";
  let store = "all";
  let sort = "name";
  let current = null; // jogo aberto no detalhe
  let alive = true;

  // ---------------- grade ----------------
  function visible() {
    let list = data.games.filter((g) => (store === "all" || g.store === store) && (!filter || g.name.toLowerCase().includes(filter)));
    if (sort === "recent") list = [...list].sort((a, b) => (b.last_played ?? 0) - (a.last_played ?? 0));
    if (sort === "size") list = [...list].sort((a, b) => (b.size_bytes ?? 0) - (a.size_bytes ?? 0));
    return list;
  }

  function cover(g) {
    const src = artCache.get(g.key + "|cover");
    return src
      ? `<img src="${src}" alt="" draggable="false">`
      : `<div class="lib-ph"><span>${esc(g.name.replace(/[^\p{L}\p{N}]/gu, "").slice(0, 2).toUpperCase() || "?")}</span></div>`;
  }

  function drawGrid() {
    current = null;
    $("search").hidden = false;
    if (!data.games.length) {
      $("body").innerHTML = `<div class="card"><div class="empty">Nenhum jogo encontrado.<div class="sub" style="margin-top:6px">
        ${data.stores.length ? `Lojas encontradas: ${esc(data.stores.join(", "))}.` : "Não achei Steam, Heroic nem Lutris neste usuário."}</div></div></div>`;
      return;
    }
    const total = data.games.reduce((s, g) => s + (g.size_bytes ?? 0), 0);
    const counts = data.stores.map((s) => [s, data.games.filter((g) => g.store === s).length]);
    const list = visible();
    // destaque: o último jogo aberto, com a arte grande da Steam
    const last = !filter && store === "all" ? [...data.games].filter((g) => g.last_played).sort((a, b) => b.last_played - a.last_played)[0] : null;
    $("body").innerHTML = `${last ? `<div class="lib-feature" id="feat">
        <div class="lib-feature-img" id="featImg"></div><div class="lib-feature-fade"></div>
        <div class="lib-feature-body">
          <div class="lib-kicker">Continuar jogando</div>
          <div class="lib-feature-name">${esc(last.name)}</div>
          <div class="sub">Jogado por último ${ago(last.last_played)}${last.playtime_min ? ` · ${playtime(last.playtime_min)} no total` : ""}</div>
          <div class="row" style="margin-top:14px"><button class="btn" type="button" data-play="${esc(last.key)}">Jogar</button>
            <button class="btn outline" type="button" data-open="${esc(last.key)}">Detalhes</button><span class="status" id="featStatus"></span></div>
        </div></div>` : ""}<div class="row lib-bar">
        <span class="sub">${data.games.length} ${data.games.length === 1 ? "jogo" : "jogos"} · ${counts.map(([s, n]) => `${esc(s)} ${n}`).join(" · ")}${total ? ` · ${bytes(total)} em disco` : ""}</span>
        <span class="spacer"></span>
        ${counts.length > 1 ? chips("store", [["all", "Todas"], ...counts.map(([s]) => [s, s])], store) : ""}
        ${chips("sort", SORTS, sort)}
      </div>
      <div class="lib-grid">${list
        .map((g) => `<button class="lib-card" type="button" data-key="${esc(g.key)}">
          <div class="lib-cover">${cover(g)}
            <div class="lib-badges">${g.platform === "Proton" || g.platform === "Wine" ? `<span class="lib-badge">${esc(g.platform)}</span>` : ""}${g.update_pending ? `<span class="lib-badge">atualizar</span>` : ""}${data.with_profile.includes(g.key) ? `<span class="lib-badge">perfil</span>` : ""}</div>
            <div class="lib-play"><span>Detalhes</span></div>
          </div>
          <div class="lib-name">${esc(g.name)}</div>
          <div class="lib-meta">${esc(g.store)}${g.size_bytes ? ` · ${bytes(g.size_bytes)}` : ""}</div>
        </button>`)
        .join("") || `<div class="empty">Nada encontrado.</div>`}</div>`;
    loadCovers(list);
    if (last) {
      art(last.key, "hero").then((src) => {
        if (src && $("featImg")) {
          $("featImg").style.backgroundImage = `url("${src}")`;
          $("feat").classList.add("has-img");
        }
      });
    }
  }

  async function loadCovers(list) {
    const missing = list.map((g) => g.key).filter((k) => !artCache.has(k + "|cover"));
    for (let i = 0; i < missing.length && alive; i += 8) {
      const part = missing.slice(i, i + 8);
      let got;
      try {
        got = await call("games", "arts", { keys: part, kind: "cover" });
      } catch {
        return;
      }
      for (const k of part) {
        artCache.set(k + "|cover", got[k] ?? null);
        const card = el.querySelector(`.lib-card[data-key="${CSS.escape(k)}"] .lib-cover`);
        if (card && got[k]) {
          const ph = card.querySelector(".lib-ph");
          const img = new Image();
          img.alt = "";
          img.draggable = false;
          img.src = got[k];
          img.className = "fade";
          if (ph) ph.replaceWith(img);
        }
      }
    }
  }

  // ---------------- detalhe ----------------
  async function art(key, kind) {
    const k = `${key}|${kind}`;
    if (!artCache.has(k)) {
      const got = await call("games", "arts", { keys: [key], kind }).catch(() => ({}));
      artCache.set(k, got[key] ?? null);
    }
    return artCache.get(k);
  }

  async function openGame(g) {
    current = { g, profile: null, line: null, presets: [], current: null };
    $("search").hidden = true;
    pageTop();
    const statsRows = [
      ["Loja", g.source],
      ["Plataforma", g.platform === "Proton" ? "Windows, pelo Proton" : g.platform === "Wine" ? "Windows, pelo Wine" : g.platform === "Nativo" ? "Nativo (Linux)" : g.platform],
      ["Tamanho", g.size_bytes ? bytes(g.size_bytes) : null],
      ["Última vez", ago(g.last_played)],
      ["Tempo jogado", playtime(g.playtime_min)],
      ["Pasta", g.install_dir],
    ];
    $("body").innerHTML = `<button class="link lib-back" id="back" type="button">← Biblioteca</button>
      <div class="lib-hero" id="hero">
        <div class="lib-hero-art"><div class="lib-hero-img" id="heroImg"></div><div class="lib-hero-logo" id="heroLogo"></div></div>
        <div class="lib-hero-body">
          <div class="row">
            <div class="lib-hero-name"><h1>${esc(g.name)}</h1>
              <div class="sub">${esc(g.store)} · ${esc(g.platform === "Nativo" ? "nativo" : g.platform)}${g.size_bytes ? ` · ${bytes(g.size_bytes)}` : ""}${g.playtime_min ? ` · ${playtime(g.playtime_min)} jogadas` : ""}</div></div>
            <span class="spacer"></span>
            ${g.update_pending ? `<span class="tag">atualização pendente</span>` : ""}
            <button class="btn outline" id="openDir" type="button">Abrir pasta</button>
            <button class="btn lib-play-btn" id="play" type="button">Jogar</button>
          </div>
          <div class="status" id="status"></div>
        </div>
      </div>
      <div class="lib-detail">
        <div class="stack"><div class="card"><h2>Informações</h2>${kv(statsRows)}</div>${toolsCard()}</div>
        <div class="card" id="opts"><h2>Opções de inicialização</h2><div class="sub">Carregando o perfil…</div></div>
      </div>`;
    $("back").addEventListener("click", drawGrid);
    $("play").addEventListener("click", async () => {
      $("play").disabled = true;
      try {
        const r = await call("games", "play", { key: g.key });
        $("status").textContent = r.message;
      } catch (e) {
        $("status").textContent = "Não abriu: " + e;
      }
      setTimeout(() => $("play") && ($("play").disabled = false), 4000);
    });
    $("openDir").addEventListener("click", () => call("games", "open_dir", { key: g.key }).catch((e) => ($("status").textContent = String(e))));

    // artes grandes chegam depois, sem segurar a tela
    art(g.key, "hero").then((src) => {
      if (current?.g !== g || !src) return;
      $("heroImg").style.backgroundImage = `url("${src}")`;
      $("hero").classList.add("has-img");
    });
    art(g.key, "logo").then((src) => {
      if (current?.g !== g || !src) return;
      $("heroLogo").innerHTML = `<img src="${src}" alt="" draggable="false">`;
    });

    try {
      const r = await call("games", "profile", { key: g.key });
      if (current?.g !== g) return;
      Object.assign(current, r);
      drawOptions();
    } catch (e) {
      $("opts").innerHTML = `<h2>Opções de inicialização</h2><div class="sub">${esc(String(e))}</div>`;
    }
  }

  function pageTop() {
    const main = document.getElementById("page");
    if (main) main.scrollTop = 0;
  }

  // cada argumento inteiro numa linha só ("-windowed" não quebra no hífen)
  const tokens = (t) => t.split(" ").map((x) => `<span class="tok">${esc(x)}</span>`).join(" ");

  function toolsCard() {
    const t = data.tools;
    const row = (on, name, what, pkg) =>
      `<div class="check"><span class="dot ${on ? "ok" : ""}"></span><div><b>${name}</b> <span class="faint small">${on ? "instalado" : "não instalado"}</span>
        <div class="sub">${what}</div>${on ? "" : `<div class="sub lib-pkg"><code>sudo pacman -S ${pkg}</code></div>`}</div></div>`;
    return `<div class="card"><h2>Ferramentas para jogos</h2>
      ${row(t.gamemode, "GameMode", "Prioridade alta e CPU em desempenho só enquanto o jogo roda.", "gamemode lib32-gamemode")}
      ${row(t.mangohud, "MangoHud", "Overlay de FPS, frametime e sensores por cima do jogo.", "mangohud lib32-mangohud")}
      ${row(t.gamescope, "Gamescope", "Compositor da Valve: resolução, escala e limite de FPS por jogo.", "gamescope")}</div>`;
  }

  function toolWarn(p) {
    const t = data.tools, w = [];
    if (p.gamemode && !t.gamemode) w.push(`O GameMode não está instalado: <code>sudo pacman -S gamemode lib32-gamemode</code>`);
    if (p.mangohud && !t.mangohud) w.push(`O MangoHud não está instalado: <code>sudo pacman -S mangohud lib32-mangohud</code>`);
    return w.map((x) => `<div class="warn-text lib-warn">${x}</div>`).join("");
  }

  function drawOptions() {
    const { g, profile: p, line, presets } = current;
    const steam = g.store === "Steam";
    const proton = g.platform === "Proton" || g.platform === "Wine";
    const cur = current.current ?? "";
    const same = (cur.trim() || "") === (line.steam || "");
    $("opts").innerHTML = `<div class="row"><h2 style="margin:0">Opções de inicialização</h2><span class="spacer"></span>
        <button class="btn outline lib-small" id="best" type="button">Perfil para mais FPS</button></div>

      <label class="field">Prioridade</label>
      ${chips("gamemode", [[false, "Normal"], [true, "Alta (GameMode)"]], p.gamemode)}
      <div class="lib-checks">
        <label class="row"><input type="checkbox" data-check="mangohud" ${p.mangohud ? "checked" : ""}> Overlay de FPS e sensores (MangoHud)</label>
        ${proton ? `<label class="row"><input type="checkbox" data-check="nvapi" ${p.nvapi ? "checked" : ""}> DLSS e Reflex no Proton (placas NVIDIA)</label>` : ""}
        <label class="row"><input type="checkbox" data-check="shader_cache" ${p.shader_cache ? "checked" : ""}> Cache de shaders da NVIDIA sem limpeza automática (menos travadas)</label>
      </div>
      ${toolWarn(p)}

      <label class="field">Modo de janela</label>${chips("win_mode", WIN_MODES, p.win_mode)}
      <label class="field">Resolução</label>${chips("res", RESES, p.res)}
      ${p.style === "source" ? `<label class="field">Taxa de atualização (Hz)</label>${chips("hz", HZS, p.hz)}` : ""}
      <label class="field">Tipo de argumento (depende do jogo)</label>${chips("style", STYLES, p.style)}
      <label class="field">Extras</label>
      <input type="text" id="extra" value="${esc(p.extra)}" placeholder="ex.: -novid +fps_max 0" style="width:100%">
      <div class="lib-presets">${presets.map((x) => `<button class="chip" type="button" data-token="${esc(x.token)}" title="${esc(x.hint)}">${esc(x.label)}</button>`).join("")}</div>
      ${p.extra.includes("-allow_third_party_software") ? `<div class="test-banner">${esc(presets.find((x) => x.token === "-allow_third_party_software")?.hint ?? "")}</div>` : ""}

      <label class="field">${steam ? "Linha para a Steam" : "Para configurar no " + esc(g.store)}</label>
      ${steam
        ? `<div class="lib-line"><code id="line">${line.steam ? tokens(line.steam) : "(nenhuma opção)"}</code>
            <button class="btn" id="copy" type="button" ${line.steam ? "" : "disabled"}>Copiar</button></div>
           <div class="sub lib-now">Na Steam agora: ${cur ? `<code>${esc(cur)}</code>` : "(nenhuma)"} ${same ? `<span class="tag ok">igual</span>` : `<span class="tag">diferente</span>`}</div>
           <p class="sub lib-note">Na Steam: botão direito no jogo → <b>Propriedades</b> → <b>Geral</b> → <b>Opções de inicialização</b>, e cole a linha.
             O app não grava nos arquivos da Steam: com ela aberta, ela sobrescreve e pode corromper a configuração.</p>`
        : `${kv([["Variáveis de ambiente", line.env.join(" ") || "—"], ["Programas antes do jogo", line.wrappers.join(" ") || "—"], ["Argumentos", line.args || "—"]])}
           <div class="row" style="margin-top:10px"><button class="btn" id="copy" type="button" ${line.args ? "" : "disabled"}>Copiar argumentos</button></div>
           <p class="sub lib-note">${g.store === "Heroic"
             ? "No Heroic: Configurações do jogo → <b>Avançado</b> (argumentos e variáveis) e <b>Outros</b> (GameMode e MangoHud)."
             : "No Lutris: botão direito no jogo → <b>Configurar</b> → <b>Opções do jogo</b> (argumentos) e <b>Opções do sistema</b> (variáveis, GameMode e MangoHud)."}</p>`}
      <div class="status" id="saved">${esc(current.note ?? "")}</div>`;
  }

  let saveTimer = null;
  function changed(delay = 0) {
    clearTimeout(saveTimer);
    const c = current;
    saveTimer = setTimeout(async () => {
      try {
        const r = await call("games", "profile_set", { key: c.g.key, profile: c.profile });
        if (current !== c) return;
        c.line = r.line;
        if (!data.with_profile.includes(c.g.key)) data.with_profile.push(c.g.key);
        const focus = document.activeElement?.id === "extra" ? $("extra").selectionStart : null;
        drawOptions();
        if (focus != null) {
          $("extra").focus();
          $("extra").setSelectionRange(focus, focus);
        }
      } catch (e) {
        $("saved") && ($("saved").textContent = String(e));
      }
    }, delay);
  }

  // ---------------- eventos ----------------
  el.addEventListener("click", async (e) => {
    const card = e.target.closest(".lib-card, [data-open]");
    if (card) return openGame(data.games.find((g) => g.key === (card.dataset.key ?? card.dataset.open)));
    const play = e.target.closest("[data-play]");
    if (play) {
      play.disabled = true;
      try {
        $("featStatus").textContent = (await call("games", "play", { key: play.dataset.play })).message;
      } catch (err) {
        $("featStatus").textContent = "Não abriu: " + err;
      }
      setTimeout(() => (play.disabled = false), 4000);
      return;
    }
    const set = e.target.closest("[data-set]");
    if (set) {
      const k = set.dataset.set, v = set.dataset.v;
      if (k === "store") (store = v), drawGrid();
      else if (k === "sort") (sort = v), drawGrid();
      else if (current?.profile) {
        current.note = null;
        current.profile[k] = k === "gamemode" ? v === "true" : k === "hz" ? Number(v) : v;
        drawOptions();
        changed();
      }
      return;
    }
    const tok = e.target.closest("[data-token]");
    if (tok && current?.profile) {
      const t = tok.dataset.token, p = current.profile;
      if (!p.extra.toLowerCase().includes(t.toLowerCase())) p.extra = (p.extra.trim() + " " + t).trim();
      drawOptions();
      changed();
      return;
    }
    if (e.target.id === "best" && current?.profile) {
      const p = current.profile;
      p.gamemode = true;
      p.shader_cache = true;
      if (current.g.platform === "Proton" || current.g.platform === "Wine") p.nvapi = true;
      current.note = "Perfil para mais FPS ligado: GameMode, cache de shaders" + (p.nvapi ? " e DLSS/Reflex no Proton." : ".");
      drawOptions();
      changed();
      return;
    }
    if (e.target.id === "copy" && current?.line) {
      const text = current.g.store === "Steam" ? current.line.steam : current.line.args;
      const ok = await copy(text);
      $("saved").textContent = ok ? "Copiado. Cole nas opções de inicialização." : "Não deu para copiar: selecione a linha e copie com Ctrl+C.";
    }
  });

  el.addEventListener("change", (e) => {
    const k = e.target.dataset?.check;
    if (k && current?.profile) {
      current.profile[k] = e.target.checked;
      drawOptions();
      changed();
    }
  });

  el.addEventListener("input", (e) => {
    if (e.target.id === "search") {
      filter = e.target.value.trim().toLowerCase();
      if (!current) drawGrid();
    } else if (e.target.id === "extra" && current?.profile) {
      current.profile.extra = e.target.value;
      changed(400);
    }
  });

  async function load() {
    $("reload").disabled = true;
    try {
      data = await call("games", "library");
      if (!alive) return;
      if (current) {
        const g = data.games.find((x) => x.key === current.g.key);
        g ? openGame(g) : drawGrid();
      } else drawGrid();
    } catch (e) {
      $("body").innerHTML = `<div class="card"><div class="empty">Não deu para ler a biblioteca: ${esc(String(e))}</div></div>`;
    } finally {
      $("reload").disabled = false;
    }
  }
  $("reload").addEventListener("click", load);

  await load();
  return () => {
    alive = false;
    clearTimeout(saveTimer);
  };
}
