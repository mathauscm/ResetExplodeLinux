// Iluminação RGB pelo OpenRGB: placa-mãe MSI (Mystic Light), memórias, periféricos Razer e o que mais
// o OpenRGB reconhecer. Os efeitos são os mesmos do app do Windows e rodam no backend.
// "Aplicar em" escolhe entre o PC inteiro e um dispositivo só: quem ganha cor própria deixa de seguir o PC.

import { invoke, head, esc, every } from "../lib.js";

const EFFECTS = [
  ["static", "Cor fixa"],
  ["breath", "Respiração"],
  ["rainbow", "Arco-íris"],
  ["cpu_temp", "Temperatura da CPU"],
  ["gpu_temp", "Temperatura da GPU"],
  ["cpu_load", "Carga da CPU"],
  ["off", "Apagar"],
];

// Paleta própria: o seletor nativo (diálogo do GTK dentro do WebKit) não responde direito no Wayland.
const PALETTE = [
  ["Vermelho", [255, 0, 0]], ["Laranja", [255, 90, 0]], ["Amarelo", [255, 200, 0]], ["Verde", [0, 255, 0]],
  ["Ciano", [0, 255, 255]], ["Azul", [0, 60, 255]], ["Roxo", [130, 0, 255]], ["Rosa", [255, 40, 160]],
  ["Magenta", [255, 0, 255]], ["Branco", [255, 255, 255]], ["Branco quente", [255, 150, 60]],
];

const ORDERS = ["rgb", "grb", "brg", "rbg", "gbr", "bgr"];
const COLOR_EFFECTS = ["static", "breath"];
const ALL = "*";

const ICONS = {
  "Memória": '<rect x="2" y="7" width="20" height="10" rx="1"/><path d="M6 17v3M10 17v3M14 17v3M18 17v3M6 10h2v4H6zM11 10h2v4h-2zM16 10h2v4h-2z"/>',
  "Placa-mãe": '<rect x="3" y="3" width="18" height="18" rx="2"/><rect x="7" y="7" width="6" height="6"/><path d="M16 7h2M16 10h2M7 16h10"/>',
  "Placa de vídeo": '<rect x="2" y="6" width="20" height="12" rx="1"/><circle cx="9" cy="12" r="3"/><path d="M15 10h4M15 14h4"/>',
  "Teclado": '<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h0M10 10h0M14 10h0M18 10h0M7 14h10"/>',
  "Mouse": '<rect x="6" y="3" width="12" height="18" rx="6"/><path d="M12 7v4"/>',
  pc: '<rect x="5" y="2" width="14" height="20" rx="2"/><circle cx="12" cy="16" r="2"/><path d="M9 6h6M9 9h6"/>',
};
const icon = (k) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${ICONS[k] ?? ICONS["Placa-mãe"]}</svg>`;

const hex = (c) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");
const same = (a, b) => a.every((v, i) => v === b[i]);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const effectName = (k) => EFFECTS.find((e) => e[0] === k)?.[1] ?? k;

function hsv(h, s, v) {
  const f = (n) => {
    const k = (n + h / 60) % 6;
    return Math.round(255 * (v - v * s * Math.max(0, Math.min(k, 4 - k, 1))));
  };
  return [f(5), f(3), f(1)];
}

function toHs([r, g, b]) {
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  let h = 0;
  if (d) h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return { h: Math.round((h * 60 + 360) % 360), s: max ? Math.round((100 * d) / max) : 0 };
}

/** Faixa de prévia de uma aparência (o que a peça mostra), em CSS. */
function strip(look) {
  const dim = (c) => c.map((v) => Math.round((v * look.brightness) / 100));
  switch (look.effect) {
    case "rainbow":
      return { bg: "linear-gradient(90deg,#f00,#ff0,#0f0,#0ff,#00f,#f0f,#f00)", cls: "flow" };
    case "breath":
      return { bg: hex(dim(look.color)), cls: "breath" };
    case "cpu_temp":
    case "gpu_temp":
      return { bg: "linear-gradient(90deg,#0040ff,#00ffff,#00ff60,#ffd000,#ff2000)", cls: "" };
    case "cpu_load":
      return { bg: "linear-gradient(90deg,#00ff00,#ffd000,#ff2000)", cls: "" };
    case "off":
      return { bg: "transparent", cls: "off" };
    default:
      return { bg: hex(dim(look.color)), cls: "" };
  }
}

export async function render(el) {
  el.innerHTML = head("RESET FERRAMENTAS", "Iluminação RGB", `<span class="tag" id="badge">desconectado</span>`) + `
    <div class="lt-scope card" id="scope"></div>
    <div class="grid wide lt-main">
      <div class="card">
        <div class="row"><h2 style="margin:0" id="editTitle">Efeito</h2><span class="spacer"></span><span class="faint small" id="editHint"></span></div>
        <div class="test-banner" id="ownBanner" hidden></div>
        <div class="chips" id="fx" style="margin-top:10px">${EFFECTS.map(([k, n]) => `<button class="chip" data-fx="${k}" type="button">${n}</button>`).join("")}</div>
        <div id="colorBox">
          <label class="field">Cor <span id="colorText"></span></label>
          <div class="palette" id="palette">${PALETTE.map(([n, c]) => `<button class="dotbtn" type="button" title="${n}" data-c="${c.join(",")}" style="background:${hex(c)}"></button>`).join("")}</div>
          <div class="lt-sliders">
            <span class="sub">Matiz</span><input type="range" id="hue" class="hue" min="0" max="359" value="0">
            <span class="sub">Saturação</span><input type="range" id="sat" class="sat" min="0" max="100" value="100">
          </div>
        </div>
        <label class="field">Brilho <span id="brightText"></span></label>
        <input type="range" id="bright" min="5" max="100" value="100">
        <div id="speedBox">
          <label class="field">Velocidade <span id="speedText"></span></label>
          <input type="range" id="speed" min="1" max="10" step="0.5" value="5">
        </div>
        <div class="live" id="live"></div>
      </div>
      <div class="card">
        <div class="row"><h2 style="margin:0">Seu PC agora</h2><span class="spacer"></span><span class="faint small">clique numa peça para mudar só ela</span></div>
        <div id="devs" style="margin-top:8px"><div class="sub">Procurando o OpenRGB…</div></div>
        <div class="status" id="msg"></div>
        <div class="row" style="margin-top:12px"><button class="btn" id="start" type="button">Ligar</button>
          <button class="btn outline" id="stop" type="button">Parar e devolver</button></div>
        <label class="row sub" style="margin-top:14px"><input type="checkbox" id="autostart"> Aplicar minhas cores ao ligar o PC</label>
        <p class="faint small" style="margin-top:6px">Ao fechar o app, os LEDs continuam como você deixou.</p>
      </div>
    </div>
    <details class="card lt-adv" id="adv">
      <summary><b>Ajustes avançados</b> <span class="faint small">zonas, número de LEDs, ordem das cores, testes, conexão</span></summary>
      <div class="row" style="margin:12px 0 4px"><button class="btn outline" id="ident" type="button">Identificar zonas</button>
        <button class="btn outline" id="test" type="button">Testar cores</button><span class="spacer"></span>
        <input type="text" id="host" value="127.0.0.1" size="12"><input type="number" id="port" value="6742" min="1" max="65535" style="width:90px">
        <button class="btn outline" id="connect" type="button">Reconectar</button></div>
      <div class="test-banner" id="banner" hidden></div>
      <div id="zones"></div>
    </details>
    <div class="grid wide" style="margin-top:12px">
      <div class="card"><h2>Detectado no seu PC</h2><div id="hw"><div class="sub">Procurando…</div></div></div>
      <div class="card"><h2>Preparar o sistema</h2><div id="env"><div class="sub">Verificando…</div></div></div>
    </div>`;

  const $ = (id) => el.querySelector("#" + id);
  let st = await invoke("light_status");
  st.settings.overrides ??= {};
  let busy = false;
  let scope = ALL; // ALL ou o nome de um dispositivo

  // ---------------- aparência por alvo ----------------
  const globalLook = () => ({ effect: st.settings.effect, color: st.settings.color, brightness: st.settings.brightness, speed: st.settings.speed });
  const lookOf = (name) => (name === ALL ? globalLook() : st.settings.overrides[name] ?? globalLook());
  const hasOwn = (name) => name !== ALL && !!st.settings.overrides[name];
  function setLook(patch) {
    if (scope === ALL) Object.assign(st.settings, patch);
    else st.settings.overrides[scope] = { ...lookOf(scope), ...patch };
  }
  const deviceLabel = (d) => {
    const sameKind = st.devices.filter((x) => x.kind === d.kind).length > 1;
    return sameKind ? d.name : d.kind;
  };

  // ---------------- desenho ----------------
  function paintStatus() {
    $("badge").textContent = st.running ? "ligado" : st.connected ? "conectado" : "desconectado";
    $("badge").classList.toggle("ok", st.running);
    if (!busy) $("msg").textContent = st.running ? "" : st.message;
    $("live").textContent = st.live;
    $("start").disabled = !st.connected || st.running;
    $("stop").disabled = !st.running;
    $("test").disabled = !st.connected || busy;
    $("ident").disabled = !st.connected || busy;
  }

  function paintScope() {
    const tab = (key, title, sub, look, ic) => {
      const p = strip(look);
      return `<button class="lt-tab${scope === key ? " on" : ""}" type="button" data-scope="${esc(key)}">
        <span class="lt-ico">${ic}</span><span class="lt-tab-txt"><b>${esc(title)}</b><span class="faint small">${esc(sub)}</span></span>
        <span class="lt-dot ${p.cls}" style="background:${p.bg}"></span></button>`;
    };
    const own = st.devices.filter((d) => hasOwn(d.name)).length;
    $("scope").innerHTML = `<span class="field" style="margin:0 6px 0 2px">Aplicar em</span>` +
      tab(ALL, "PC inteiro", own ? `${st.devices.length - own} de ${st.devices.length} seguem` : "tudo junto", globalLook(), icon("pc")) +
      st.devices.map((d) => tab(d.name, deviceLabel(d), hasOwn(d.name) ? "cor própria" : "segue o PC", lookOf(d.name), icon(d.kind))).join("");
  }

  function paintControls() {
    const look = lookOf(scope);
    const d = st.devices.find((x) => x.name === scope);
    $("editTitle").textContent = scope === ALL ? "Efeito do PC inteiro" : `Efeito · ${d ? deviceLabel(d) : scope}`;
    $("editHint").textContent = scope === ALL ? "" : hasOwn(scope) ? "esta peça tem cor própria" : "mudar aqui dá uma cor só para esta peça";
    const ownNames = st.devices.filter((x) => hasOwn(x.name)).map(deviceLabel);
    const ob = $("ownBanner");
    if (scope === ALL && ownNames.length) {
      ob.hidden = false;
      ob.innerHTML = `<b>${esc(ownNames.join(", "))}</b> ${ownNames.length > 1 ? "têm" : "tem"} cor própria e não muda${ownNames.length > 1 ? "m" : ""} junto. <button class="link" type="button" id="syncAll">Igualar todos ao PC inteiro</button>`;
    } else if (hasOwn(scope)) {
      ob.hidden = false;
      ob.innerHTML = `Só esta peça usa esta cor. <button class="link" type="button" id="followAll">Voltar a seguir o PC inteiro</button>`;
    } else ob.hidden = true;

    el.querySelectorAll("#fx .chip").forEach((c) => c.classList.toggle("on", c.dataset.fx === look.effect));
    el.querySelectorAll("#palette .dotbtn").forEach((b) => b.classList.toggle("on", same(b.dataset.c.split(",").map(Number), look.color)));
    const { h, s: sat } = toHs(look.color);
    if (document.activeElement !== $("hue")) $("hue").value = h;
    if (document.activeElement !== $("sat")) $("sat").value = sat;
    $("sat").style.setProperty("--hue-color", hex(hsv(+$("hue").value, 1, 1)));
    $("colorText").textContent = `· RGB ${look.color.join(", ")}`;
    $("colorBox").classList.toggle("muted-box", !COLOR_EFFECTS.includes(look.effect) && look.effect !== "off");
    $("bright").value = look.brightness;
    $("brightText").textContent = `· ${look.brightness}%`;
    $("speed").value = look.speed;
    $("speedText").textContent = `· ${String(look.speed).replace(".", ",")}`;
    $("speedBox").hidden = !["breath", "rainbow"].includes(look.effect);
  }

  function paintDevices() {
    const s = st.settings;
    $("devs").innerHTML = st.devices.length
      ? st.devices.map((d) => {
          const look = lookOf(d.name);
          const p = strip(look);
          const on = s.selected.includes(d.id);
          return `<div class="lt-dev${scope === d.name ? " on" : ""}${on ? "" : " disabled"}" data-pick="${esc(d.name)}">
            <span class="lt-ico">${icon(d.kind)}</span>
            <div class="lt-dev-main"><div class="row" style="gap:6px"><b>${esc(deviceLabel(d))}</b>${hasOwn(d.name) ? '<span class="tag">cor própria</span>' : ""}</div>
              <div class="faint small ellipsis">${esc(d.name)} · ${d.led_count ?? d.zones.reduce((a, z) => a + z.leds, 0)} LEDs · ${esc(on ? effectName(look.effect) : "não controlado pelo app")}</div>
              <div class="lt-strip ${p.cls}" style="background:${on ? p.bg : "transparent"}"></div></div>
            <label class="switch" title="${on ? "Desligar desta peça (volta ao efeito de antes)" : "Controlar esta peça"}"><input type="checkbox" data-id="${esc(d.id)}" ${on ? "checked" : ""}><span></span></label>
          </div>`;
        }).join("")
      : `<div class="sub">${st.connected ? "O OpenRGB não encontrou dispositivos com LEDs." : "Os dispositivos aparecem aqui assim que o OpenRGB responder."}</div>`;

    $("zones").innerHTML = st.devices.map((d) => `<div class="lt-zdev"><b>${esc(d.name)}</b> <span class="faint small">${esc(d.detail)}</span>
      ${d.zones.map((z, i) => `<div class="zone"><span class="zname">${esc(z.name)}${z.resizable ? ' <span class="faint">ARGB</span>' : ""}</span>
        ${z.resizable ? `<input type="number" min="${z.min}" max="${z.max}" value="${z.leds}" data-dev="${esc(d.id)}" data-zone="${i}" title="LEDs neste conector"><span class="faint">LEDs</span>` : `<span class="faint">${z.leds} LED${z.leds === 1 ? "" : "s"}</span>`}
        <button class="link" type="button" data-ident="${i}" data-dev="${esc(d.id)}" title="Acende só esta zona, em branco, por 4 s">identificar</button>
        <select data-dev="${esc(d.id)}" data-order="${i}" title="Ordem das cores (use se a cor sair trocada)">${ORDERS.map((o) => `<option value="${o}" ${o === z.order ? "selected" : ""}>${o.toUpperCase()}</option>`).join("")}</select>
        ${z.resizable && z.leds === 0 ? '<span class="warn-text">0 LEDs: o que estiver ligado aqui fica apagado</span>' : ""}</div>`).join("")}</div>`).join("");
  }

  function paint() {
    paintStatus();
    paintScope();
    paintControls();
    paintDevices();
  }

  // ---------------- envio ----------------
  // arrastar um controle gera dezenas de mudanças: manda só a última a cada 150 ms (o backend ainda segura
  // a memória num ritmo que o SMBus aguenta) e liga o efeito sozinho, sem precisar do botão
  let pending = null;
  function push() {
    paintScope();
    paintControls();
    paintDevices();
    clearTimeout(pending);
    pending = setTimeout(async () => {
      st = { ...(await invoke("light_apply", { settings: st.settings })), settings: st.settings };
      if (st.connected && !st.running) st = { ...(await invoke("light_start")), settings: st.settings };
      paintStatus();
    }, 150);
  }

  function setColor(c) {
    const look = lookOf(scope);
    setLook({ color: c, ...(COLOR_EFFECTS.includes(look.effect) ? {} : { effect: "static" }) });
    push();
  }

  $("scope").addEventListener("click", (e) => {
    const b = e.target.closest("[data-scope]");
    if (!b) return;
    scope = b.dataset.scope;
    paintScope();
    paintControls();
    paintDevices();
  });
  $("fx").addEventListener("click", (e) => {
    const b = e.target.closest(".chip");
    if (!b) return;
    setLook({ effect: b.dataset.fx });
    push();
  });
  $("palette").addEventListener("click", (e) => {
    const b = e.target.closest(".dotbtn");
    if (b) setColor(b.dataset.c.split(",").map(Number));
  });
  const fromSliders = () => setColor(hsv(+$("hue").value, +$("sat").value / 100, 1));
  $("hue").addEventListener("input", fromSliders);
  $("sat").addEventListener("input", fromSliders);
  $("bright").addEventListener("input", (e) => {
    setLook({ brightness: +e.target.value });
    push();
  });
  $("speed").addEventListener("input", (e) => {
    setLook({ speed: +e.target.value });
    push();
  });
  $("ownBanner").addEventListener("click", (e) => {
    if (e.target.id === "syncAll") st.settings.overrides = {};
    else if (e.target.id === "followAll") delete st.settings.overrides[scope];
    else return;
    push();
  });

  // clicar numa peça escolhe ela como alvo; o interruptor liga/desliga o controle dela
  $("devs").addEventListener("click", (e) => {
    if (e.target.closest(".switch")) return;
    const row = e.target.closest("[data-pick]");
    if (!row) return;
    scope = scope === row.dataset.pick ? ALL : row.dataset.pick;
    paintScope();
    paintControls();
    paintDevices();
  });
  $("devs").addEventListener("change", (e) => {
    const id = e.target.dataset.id;
    if (!id) return;
    const sel = new Set(st.settings.selected);
    e.target.checked ? sel.add(id) : sel.delete(id);
    st.settings.selected = [...sel];
    push();
  });

  // ---------------- ajustes avançados ----------------
  function showBanner(html) {
    $("banner").hidden = false;
    $("banner").innerHTML = html;
  }

  $("zones").addEventListener("click", async (e) => {
    const b = e.target.closest("[data-ident]");
    if (!b) return;
    const d = st.devices.find((x) => x.id === b.dataset.dev);
    showBanner(`Acesa em <b>branco</b>: só <b>${esc(d?.name ?? "")} · ${esc(d?.zones[+b.dataset.ident]?.name ?? "")}</b>. Todos os LEDs dela acenderam?`);
    st = { ...(await invoke("light_identify", { device: b.dataset.dev, zone: +b.dataset.ident })), settings: st.settings };
    paintStatus();
  });

  $("zones").addEventListener("change", async (e) => {
    const t = e.target;
    if (t.dataset.zone != null) {
      const size = Math.max(+t.min, Math.min(+t.max, Math.round(+t.value || 0)));
      st = { ...(await invoke("light_resize", { device: t.dataset.dev, zone: +t.dataset.zone, size })), settings: st.settings };
      paint();
    } else if (t.dataset.order != null) {
      st = { ...(await invoke("light_order", { device: t.dataset.dev, zone: +t.dataset.order, order: t.value })), settings: st.settings };
      paint();
    }
  });

  // percorre todas as zonas, uma de cada vez: mostra o que é cada uma e se algum LED não acende
  $("ident").addEventListener("click", async () => {
    busy = true;
    paintStatus();
    try {
      for (const d of st.devices)
        for (const [i, z] of d.zones.entries()) {
          showBanner(`Acesa em <b>branco</b>: <b>${esc(d.name)} · ${esc(z.name)}</b> (${z.leds} LED${z.leds === 1 ? "" : "s"}). Repare se acendeu inteira.`);
          await invoke("light_identify", { device: d.id, zone: i });
          await sleep(4200);
        }
      showBanner("Pronto. Alguma zona não acendeu, ou acendeu só em parte? Se for um conector ARGB, aumente os LEDs dela. Se não acendeu nada, provavelmente não há nada ligado naquele conector.");
    } finally {
      busy = false;
      paint();
    }
  });

  // Mostra vermelho, verde e azul puros em tudo (ignorando as cores próprias). Se uma zona mostrar outra
  // cor, a ordem dela está errada.
  $("test").addEventListener("click", async () => {
    busy = true;
    paintStatus();
    const keep = structuredClone(st.settings);
    const wasRunning = st.running;
    try {
      for (const [name, c] of [["VERMELHO", [255, 0, 0]], ["VERDE", [0, 255, 0]], ["AZUL", [0, 0, 255]]]) {
        showBanner(`Agora todos os LEDs deveriam estar <b style="color:${hex(c)}">${name}</b>`);
        await invoke("light_apply", { settings: { ...keep, effect: "static", color: c, brightness: 100, overrides: {} } });
        if (!st.running) st = { ...(await invoke("light_start")), settings: keep };
        await sleep(3000);
      }
      showBanner(`Alguma zona mostrou a cor errada? Mude a ordem dela na lista abaixo (no vermelho apareceu verde → <b>GRB</b>; no vermelho apareceu azul → <b>BRG</b>) e teste de novo.`);
    } finally {
      st = { ...(await invoke("light_apply", { settings: keep })), settings: keep };
      if (!wasRunning) st = { ...(await invoke("light_stop")), settings: keep };
      busy = false;
      paint();
    }
  });

  // ---------------- conexão (automática) ----------------
  // ao abrir a tela; se o OpenRGB estiver fechado, o backend abre o servidor e espera a detecção
  async function connect() {
    busy = true;
    $("connect").disabled = true;
    $("msg").textContent = "Procurando o OpenRGB… se ele estiver fechado, o app abre e espera a detecção dos dispositivos (até 20 s).";
    try {
      const keep = st.settings;
      st = await invoke("light_ensure", { host: $("host").value.trim() || "127.0.0.1", port: +$("port").value || 6742 });
      if (st.connected) {
        const settings = { ...keep, selected: st.settings.selected };
        st = { ...(await invoke("light_apply", { settings })), settings };
      }
      st.settings.overrides ??= {};
    } finally {
      busy = false;
      $("connect").disabled = false;
      paint();
    }
  }
  $("connect").addEventListener("click", connect);
  $("start").addEventListener("click", async () => {
    st = { ...(await invoke("light_start")), settings: st.settings };
    paintStatus();
  });
  $("stop").addEventListener("click", async () => {
    st = { ...(await invoke("light_stop")), settings: st.settings };
    paintStatus();
  });

  // ---------------- hardware detectado e checklist ----------------
  Promise.all([invoke("usb_list"), invoke("gpu_list")]).then(([usb, gpus]) => {
    let html = usb.filter((d) => d.rgb).map((d) => `<div class="check"><span class="dot ok"></span><div><b>${esc(d.rgb.kind)}</b><div class="sub">USB ${d.vid}:${d.pid} · controle via ${esc(d.rgb.via)}</div></div></div>`).join("");
    if (!usb.some((d) => d.vid === "1532"))
      html += `<div class="check"><span class="dot warn"></span><div><b>Nenhum periférico Razer conectado agora</b><div class="sub">Conecte o mouse/teclado Razer pelo cabo ou receptor USB: o OpenRGB controla os LEDs por USB (por Bluetooth não dá).</div></div></div>`;
    for (const g of gpus)
      html += `<div class="check"><span class="dot"></span><div><b>${esc(g.name)}</b><div class="sub">LEDs de placa de vídeo dependem do fabricante (subsistema ${esc(g.pci_subsystem_id ?? "?")}). Se a sua tiver RGB, o OpenRGB a lista pelo I²C.</div></div></div>`;
    $("hw").innerHTML = html;
  });

  invoke("light_env").then((env) => {
    const step = (ok, title, how, optional = false) =>
      `<div class="check"><span class="dot ${ok ? "ok" : optional ? "" : "bad"}"></span><div><b>${title}</b>${ok ? "" : `<div class="sub">${how}</div>`}</div></div>`;
    $("env").innerHTML = [
      step(env.openrgb_installed, "OpenRGB instalado", "<code>sudo pacman -S openrgb</code>"),
      step(env.openrgb_udev, "Regras udev do OpenRGB", "Vêm com o pacote do OpenRGB. Depois de instalar, reinicie ou rode <code>sudo udevadm control --reload &amp;&amp; sudo udevadm trigger</code>."),
      step(env.i2c_dev, "I²C liberado (memórias e GPU RGB)", "<code>sudo modprobe i2c-dev</code> e, para valer sempre, <code>echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf</code>.", true),
      step(env.openrazer_installed, "OpenRazer (opcional)", "Só se quiser o software da Razer em paralelo. O OpenRGB já controla os LEDs Razer sozinho; os dois juntos brigam pelo dispositivo.", true),
    ].join("");
  });

  invoke("light_autostart").then((on) => ($("autostart").checked = on));
  $("autostart").addEventListener("change", async (e) => {
    try {
      e.target.checked = await invoke("light_set_autostart", { on: e.target.checked });
    } catch (err) {
      e.target.checked = !e.target.checked;
      $("msg").textContent = `Não deu para mudar a opção de iniciar com o sistema: ${err}`;
    }
  });

  paint();
  if (!st.connected) connect();
  // acompanha o valor ao vivo (temperatura/carga) e quedas de conexão
  return every(1000, async () => {
    if (busy) return;
    st = { ...(await invoke("light_status")), settings: st.settings };
    paintStatus();
  });
}
