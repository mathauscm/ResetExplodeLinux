// Overlay: no Linux quem desenha o FPS e os sensores por cima do jogo é o MangoHud (roda dentro do jogo).
// Esta tela escolhe o que aparece (os mesmos itens do app do Windows), grava o MangoHud.conf e mostra uma
// pré-visualização com os sensores reais do PC (o FPS é simulado: só o jogo sabe o FPS dele).

import { call, head, esc, snapshot, every } from "../lib.js";
import { runJob } from "../pkgjob.js";

const GROUPS = ["FPS", "GPU", "VRAM", "CPU", "RAM", "Outros"];
const CORNERS = [["TL", "Superior esquerdo"], ["TR", "Superior direito"], ["BL", "Inferior esquerdo"], ["BR", "Inferior direito"]];
const SIZES = [["small", "Pequeno"], ["medium", "Médio"], ["large", "Grande"]];
const PRESETS = [["min", "Só FPS"], ["default", "Padrão"], ["all", "Tudo"]];

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const keyName = (k) => k.replace(/_[LR]\b/g, "").replace("Control", "Ctrl").split("+").map((p) => `<kbd>${esc(p)}</kbd>`).join("+");

export async function render(el) {
  el.innerHTML = head("RESET JOGOS", "Overlay", `<span class="tag" id="mhTag">verificando…</span>`) + `<div class="empty">Lendo a configuração…</div>`;
  let st = await call("overlay", "state");
  let cur = structuredClone(st.conf.settings);
  let saved = st.conf.ours ? structuredClone(st.conf.settings) : null;
  let keys = [];

  el.innerHTML = head("RESET JOGOS", "Overlay", `<span class="tag" id="mhTag"></span>`) + `
    <p class="sub ov-lead">No Linux o FPS dos jogos é lido pelo <b class="fg">MangoHud</b>, que roda dentro do jogo (Vulkan, OpenGL e Proton).
      Escolha aqui o que ele mostra; o app grava o <code>MangoHud.conf</code> e o MangoHud aplica na hora, mesmo com o jogo aberto.</p>
    <div class="ov-top">
      <div class="card">
        <div class="row"><h2 style="margin:0">O que mostrar</h2><span class="spacer"></span>
          <div class="chips" id="presets">${PRESETS.map(([k, n]) => `<button class="chip" type="button" data-preset="${k}">${n}</button>`).join("")}</div></div>
        <div id="items"></div>
        <div class="ov-opts">
          <div><label class="field">Posição</label>
            <div class="ov-corners" id="corners">${CORNERS.map(([k, n]) => `<button type="button" data-corner="${k}" title="${n}" aria-label="${n}"><i></i></button>`).join("")}</div></div>
          <div><label class="field">Tamanho</label>
            <div class="chips" id="sizes">${SIZES.map(([k, n]) => `<button class="chip" type="button" data-size="${k}">${n}</button>`).join("")}</div></div>
          <div><label class="field">Mostrar / esconder no jogo</label>
            <select id="toggle">${st.toggles.map((t) => `<option value="${esc(t.key)}">${esc(t.label)}</option>`).join("")}</select>
            <label class="row sub" style="margin-top:8px"><input type="checkbox" id="hidden"> Começar escondido (aparece com o atalho)</label></div>
        </div>
      </div>
      <div class="card ov-prev">
        <div class="row"><h2 style="margin:0">Pré-visualização</h2><span class="spacer"></span><span class="faint small">FPS simulado · sensores reais</span></div>
        <div class="ov-screen" id="screen"><div class="ov-hud" id="hud"></div><div class="ov-hint" id="hint"></div></div>
        <div class="row" style="margin-top:12px">
          <button class="btn" id="save" type="button">Salvar no MangoHud</button>
          <button class="btn outline" id="restore" type="button">Restaurar original</button>
          <span class="spacer"></span><span class="tag" id="saveTag"></span></div>
        <div class="test-banner" id="note" hidden></div>
        <div class="status" id="msg"></div>
        <details class="adv"><summary>Ver o MangoHud.conf que será gravado</summary>
          <pre class="ov-conf" id="conf"></pre><div class="faint small mono" id="confPath"></div></details>
      </div>
    </div>
    <div class="grid wide" style="margin-top:12px">
      <div class="card"><h2>MangoHud</h2><div id="install"></div></div>
      <div class="card"><h2>Ligar nos jogos</h2><div id="launch"></div></div>
    </div>
    <div class="card job" id="job" hidden style="margin-top:12px"></div>`;

  const $ = (id) => el.querySelector("#" + id);

  // ---------------- escolhas ----------------
  function paintItems() {
    $("items").innerHTML = GROUPS.map((g) => {
      const list = st.catalog.filter((m) => m.group === g);
      const notes = list.filter((m) => m.note && cur.items.includes(m.key));
      return `<div class="ov-group"><div class="ov-gname">${g}</div><div>
        <div class="chips">${list.map((m) => `<button class="chip${cur.items.includes(m.key) ? " on" : ""}" type="button" data-key="${m.key}">${esc(m.label)}${m.note ? '<span class="ov-star">*</span>' : ""}</button>`).join("")}</div>
        ${notes.length ? `<div class="ov-notes">${notes.map((m) => `<div><b>${esc(m.label)}:</b> ${esc(m.note)}</div>`).join("")}</div>` : ""}
      </div></div>`;
    }).join("");
    el.querySelectorAll("#presets .chip").forEach((c) => c.classList.toggle("on", same(st.presets[c.dataset.preset], cur.items)));
  }

  function paintOpts() {
    el.querySelectorAll("#corners button").forEach((b) => b.classList.toggle("on", b.dataset.corner === cur.corner));
    el.querySelectorAll("#sizes .chip").forEach((c) => c.classList.toggle("on", c.dataset.size === cur.size));
    $("toggle").value = cur.toggle;
    $("hidden").checked = cur.start_hidden;
    $("screen").dataset.corner = cur.corner;
    $("hud").dataset.size = cur.size;
    $("hint").innerHTML = `${keyName(cur.toggle)} mostra / esconde${cur.start_hidden ? " · começa escondido" : ""}`;
  }

  function paintSave() {
    const c = st.conf;
    const clean = saved && same(saved, cur) && !c.edited;
    $("saveTag").textContent = clean ? "salvo" : c.exists && !c.ours ? "arquivo seu, não do app" : "não salvo";
    $("saveTag").classList.toggle("ok", !!clean);
    $("save").disabled = !!clean;
    $("restore").disabled = !(c.backup_exists || c.ours);
    $("restore").title = c.backup_exists ? "Volta o MangoHud.conf que existia antes do app" : "Apaga o arquivo do app: o MangoHud volta ao visual padrão";
    const note = $("note");
    note.hidden = !(c.exists && (!c.ours || c.edited));
    note.innerHTML = !c.ours
      ? `Você já tem um <code>MangoHud.conf</code> próprio. Ao salvar, o app guarda uma cópia dele (<code>MangoHud.conf.antes-do-reset-explode</code>) e você volta a ele com <b>Restaurar original</b>.`
      : `O arquivo gerado pelo app foi mudado à mão. Ao salvar, essa versão ganha uma cópia antes de ser trocada.`;
    $("confPath").textContent = st.paths.conf;
  }

  let previewSeq = 0;
  async function refresh() {
    paintItems();
    paintOpts();
    paintSave();
    const seq = ++previewSeq;
    const p = await call("overlay", "preview", { settings: cur });
    if (seq !== previewSeq) return;
    keys = p.keys;
    $("conf").textContent = p.text;
    drawHud(lastSnap);
  }

  el.addEventListener("click", (e) => {
    const t = e.target.closest("button");
    if (!t) return;
    if (t.dataset.key) {
      const k = t.dataset.key;
      cur.items = cur.items.includes(k) ? cur.items.filter((x) => x !== k) : [...cur.items, k];
      cur.items = st.catalog.map((m) => m.key).filter((x) => cur.items.includes(x));
    } else if (t.dataset.preset) cur.items = [...st.presets[t.dataset.preset]];
    else if (t.dataset.corner) cur.corner = t.dataset.corner;
    else if (t.dataset.size) cur.size = t.dataset.size;
    else return;
    refresh();
  });
  $("toggle").addEventListener("change", () => {
    cur.toggle = $("toggle").value;
    paintLaunch(); // o atalho aparece também no cartão "Ligar nos jogos"
    refresh();
  });
  $("hidden").addEventListener("change", () => ((cur.start_hidden = $("hidden").checked), refresh()));

  $("save").addEventListener("click", async () => {
    try {
      const r = await call("overlay", "save", { settings: cur });
      st.conf = r.conf;
      saved = structuredClone(cur);
      $("msg").textContent = r.backup
        ? `Salvo. O arquivo anterior foi guardado em ${r.backup}.`
        : "Salvo. Se um jogo estiver aberto com o MangoHud, ele já mudou.";
    } catch (err) {
      $("msg").textContent = String(err);
    }
    paintSave();
  });

  $("restore").addEventListener("click", async () => {
    try {
      const r = await call("overlay", "restore");
      st.conf = r.conf;
      saved = r.conf.ours ? structuredClone(r.conf.settings) : null;
      $("msg").textContent = r.message;
    } catch (err) {
      $("msg").textContent = String(err);
    }
    paintSave();
  });

  // ---------------- instalação e como ligar ----------------
  function paintInstall() {
    const i = st.install;
    $("mhTag").textContent = i.installed ? `MangoHud ${i.version ?? "instalado"}` : "MangoHud não instalado";
    $("mhTag").classList.toggle("ok", i.installed);
    const check = (dot, title, sub) => `<div class="check"><span class="dot ${dot}"></span><div><b>${title}</b>${sub ? `<div class="sub">${sub}</div>` : ""}</div></div>`;
    $("install").innerHTML =
      check(i.installed ? "ok" : "bad", i.installed ? `Instalado${i.version ? ` · ${esc(i.version)}` : ""}` : "Não instalado",
        i.installed ? "Pacote <code>mangohud</code> do repositório oficial." : "Sem ele não há overlay nos jogos. A instalação usa o pacman e pede a senha.") +
      (i.multilib
        ? check(i.lib32 ? "ok" : "warn", i.lib32 ? "Versão de 32 bits instalada" : "Falta a versão de 32 bits", "<code>lib32-mangohud</code>: jogos antigos e alguns do Proton são de 32 bits.")
        : check("", "Repositório multilib desligado", "Sem a versão de 32 bits; a maioria dos jogos atuais é de 64 bits e funciona.")) +
      `<div class="row" style="margin-top:12px">
        ${i.packages.length ? `<button class="btn" id="install-btn" type="button">${i.installed ? "Instalar 32 bits" : "Instalar MangoHud"}</button>` : ""}
        ${i.can_test ? `<button class="btn outline" id="test-btn" type="button">Testar com vkcube</button>` : ""}
        ${!i.packages.length && !i.can_test ? `<span class="sub">Pronto para usar.</span>` : ""}
      </div>`;
    $("install-btn")?.addEventListener("click", async (e) => {
      e.target.disabled = true;
      await runJob({ kind: "install", packages: i.packages }, $("job"), `Instalando ${i.packages.join(", ")}`);
      st = await call("overlay", "state");
      paintInstall();
      paintLaunch();
    });
    $("test-btn")?.addEventListener("click", async () => {
      try {
        await call("overlay", "test");
        $("msg").textContent = "Abrindo o vkcube com o MangoHud…";
      } catch (err) {
        $("msg").textContent = String(err);
      }
    });
  }

  function paintLaunch() {
    const g = st.global;
    const pending = g.enabled !== g.active_now;
    $("launch").innerHTML = `
      <label class="row"><input type="checkbox" id="global" ${g.enabled ? "checked" : ""} ${st.install.installed ? "" : "disabled"}>
        <b>Em todos os jogos Vulkan</b></label>
      <p class="sub" style="margin:6px 0 0 25px">Cria <code>${esc(g.path.replace(/^.*\/\.config\//, "~/.config/"))}</code> com <code>MANGOHUD=1</code>.
        Vale para jogos Vulkan e Proton (DXVK / VKD3D). Pode aparecer também em outros programas que usam Vulkan; aí use o atalho para esconder.</p>
      ${pending ? `<div class="test-banner">${g.enabled ? "Ligado. Vale a partir do <b>próximo login</b>: saia da sessão e entre de novo." : "Desligado. A sessão atual ainda tem <code>MANGOHUD=1</code> até você sair e entrar de novo."}</div>` : ""}
      <div class="ov-how">
        <div><span class="ov-step">Steam</span><div>Propriedades do jogo → Opções de inicialização:
          <div class="row" style="margin-top:6px"><code id="steamCmd">mangohud %command%</code><button class="link" type="button" id="copy">copiar</button></div>
          <div class="faint small" style="margin-top:4px">Funciona também em jogos OpenGL e nativos, que o modo "todos os jogos" não pega.</div></div></div>
        <div><span class="ov-step">Lutris / Heroic</span><div>Opção <b>MangoHud</b> nas configurações do jogo (ou do executor).</div></div>
        <div><span class="ov-step">Terminal</span><div><code>mangohud ./jogo</code></div></div>
        <div><span class="ov-step">No jogo</span><div>${keyName(cur.toggle)} mostra ou esconde o overlay (funciona nos jogos via XWayland, como os do Proton).</div></div>
      </div>
      ${st.install.installed ? "" : `<p class="faint small" style="margin-top:10px">Instale o MangoHud para ligar.</p>`}`;
    $("global")?.addEventListener("change", async (e) => {
      try {
        st.global = await call("overlay", "global", { on: e.target.checked });
      } catch (err) {
        $("msg").textContent = String(err);
      }
      paintLaunch();
    });
    $("copy").addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText("mangohud %command%");
        $("copy").textContent = "copiado";
      } catch {
        $("copy").textContent = "selecione e copie";
      }
    });
  }

  // ---------------- pré-visualização ----------------
  // FPS simulado: passeia perto de 144 com um engasgo de vez em quando, para o gráfico ter cara de jogo
  const ft = Array.from({ length: 90 }, () => 6.9 + Math.random() * 0.6);
  function tickFps() {
    const spike = Math.random() < 0.05 ? 6 + Math.random() * 10 : 0;
    ft.push(6.6 + Math.random() * 0.9 + spike);
    ft.shift();
  }
  const fpsNow = () => 1000 / (ft.slice(-10).reduce((a, b) => a + b, 0) / 10);
  const low1 = () => 1000 / [...ft].sort((a, b) => b - a)[Math.max(0, Math.floor(ft.length * 0.01))];

  function graph() {
    const w = 200, h = 34, max = Math.max(...ft, 20);
    const pts = ft.map((v, i) => `${((i / (ft.length - 1)) * w).toFixed(1)},${(h - 1 - (h - 3) * Math.min(1, v / max)).toFixed(1)}`).join(" ");
    return `<svg class="ov-graph" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none"><line x1="0" x2="${w}" y1="${h - 0.5}" y2="${h - 0.5}"/><polyline points="${pts}"/></svg>`;
  }

  let lastSnap = null;
  // números como o MangoHud escreve: sem separador de milhar ("4650 MHz")
  const n = (val, d = 0) => (val == null ? "—" : val.toFixed(d).replace(".", ","));
  const v = (val, unit, d = 0) => `<span class="ov-v">${n(val, d)}<u>${unit}</u></span>`;
  const gib = (b) => (b == null ? null : b / 1024 ** 3);

  function drawHud(snap) {
    const has = (k) => keys.includes(k);
    const s = snap?.summary ?? {};
    const g = s.gpu ?? {};
    const rows = [];
    const row = (label, cells, cls = "") => rows.push(`<div class="ov-row ${cls}"><span class="ov-l">${label}</span>${cells.join("")}</div>`);

    // mesma ordem do arquivo (e do MangoHud): FPS, sensores, nome do jogo e hora
    if (has("fps")) {
      const c = [`<span class="ov-v ov-fps">${n(fpsNow())}<u>FPS</u></span>`];
      if (has("frametime")) c.push(v(ft.at(-1), "ms", 1));
      row("DXVK", c);
    }
    if (has("fps_metrics")) row("1% low", [v(low1(), "FPS")]);
    if (has("frame_timing")) rows.push(`<div class="ov-row"><span class="ov-l">Frametime</span>${graph()}${v(ft.at(-1), "ms", 1)}</div>`);
    if (has("gpu_stats")) {
      const c = [v(g.load_pct, "%")];
      if (has("gpu_temp")) c.push(v(g.temp_c, "°C"));
      if (has("gpu_junction_temp")) c.push(v(null, "°C"));
      if (has("gpu_core_clock")) c.push(v(g.core_mhz, "MHz"));
      if (has("gpu_power")) c.push(v(g.power_w, "W"));
      if (has("gpu_voltage")) c.push(v(null, "mV"));
      if (has("gpu_fan")) c.push(v(g.fan_pct, "%"));
      row("GPU", c);
    }
    if (has("throttling_status")) {
      const cap = (g.perfcap ?? []).filter((x) => x !== "Ocioso");
      row("Limite", [`<span class="ov-v ov-dim">${cap.length ? esc(cap.join(", ")) : "só quando limitar"}</span>`]);
    }
    if (has("vram")) {
      const c = [v(gib(g.vram_used), "GiB", 1)];
      if (has("gpu_mem_clock")) c.push(v(g.mem_mhz, "MHz"));
      if (has("gpu_mem_temp")) c.push(v(null, "°C"));
      row("VRAM", c);
    }
    if (has("cpu_stats")) {
      const c = [v(s.cpu_load_pct, "%")];
      if (has("cpu_temp")) c.push(v(s.cpu_temp_c, "°C"));
      if (has("cpu_mhz")) c.push(v(s.cpu_mhz_max, "MHz"));
      if (has("cpu_power")) c.push(v(null, "W"));
      row("CPU", c);
    }
    if (has("core_load")) {
      const cores = (snap?.rows ?? []).filter((r) => /^cpu\/load\/\d+$/.test(r.id)).map((r) => r.value);
      rows.push(`<div class="ov-row"><span class="ov-l">Núcleos</span><span class="ov-bars">${cores.map((c) => `<i style="height:${Math.max(6, c)}%"></i>`).join("")}</span></div>`);
    }
    if (has("ram")) row("RAM", [v(gib(s.ram_used), "GiB", 1)]);
    if (has("exec_name")) rows.push(`<div class="ov-row ov-dim">cs2</div>`);
    if (has("time")) rows.push(`<div class="ov-row ov-dim">${new Date().toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" })}</div>`);
    $("hud").innerHTML = rows.length ? rows.join("") : `<div class="ov-row ov-dim">nada escolhido</div>`;
  }

  paintInstall();
  paintLaunch();
  await refresh();

  return every(1000, async () => {
    tickFps();
    lastSnap = await snapshot();
    drawHud(lastSnap);
  });
}
