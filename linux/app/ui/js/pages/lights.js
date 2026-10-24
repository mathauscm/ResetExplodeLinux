// Iluminação RGB pelo OpenRGB: placa-mãe MSI (Mystic Light), memórias, periféricos Razer e o que mais
// o OpenRGB reconhecer. Os efeitos são os mesmos do app do Windows e rodam no backend.

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

const hex = (c) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");
const same = (a, b) => a.every((v, i) => v === b[i]);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

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

export async function render(el) {
  el.innerHTML = head("RESET FERRAMENTAS", "Iluminação RGB", `<span class="tag" id="badge">desconectado</span>`) + `
    <div class="grid wide">
      <div class="card">
        <div class="row"><h2 style="margin:0">Dispositivos</h2><span class="spacer"></span>
          <button class="btn outline" id="ident" type="button">Identificar zonas</button>
          <button class="btn outline" id="test" type="button">Testar cores</button></div>
        <div class="status" id="msg"></div>
        <div class="test-banner" id="banner" hidden></div>
        <div id="devs" style="margin-top:6px"><div class="sub">Procurando o OpenRGB…</div></div>
        <details class="adv"><summary>Conexão</summary>
          <div class="row" style="margin-top:8px"><input type="text" id="host" value="127.0.0.1" size="12"><input type="number" id="port" value="6742" min="1" max="65535" style="width:90px">
          <button class="btn outline" id="connect" type="button">Reconectar</button></div>
        </details>
      </div>
      <div class="card">
        <h2>Efeito</h2>
        <div class="chips" id="fx">${EFFECTS.map(([k, n]) => `<button class="chip" data-fx="${k}" type="button">${n}</button>`).join("")}</div>
        <label class="field">Cor <span id="colorText"></span></label>
        <div class="palette" id="palette">${PALETTE.map(([n, c]) => `<button class="dotbtn" type="button" title="${n}" data-c="${c.join(",")}" style="background:${hex(c)}"></button>`).join("")}</div>
        <div class="row" style="margin-top:10px"><span class="sub" style="width:70px">Matiz</span><input type="range" id="hue" class="hue" min="0" max="359" value="0" style="flex:1"></div>
        <div class="row"><span class="sub" style="width:70px">Saturação</span><input type="range" id="sat" min="0" max="100" value="100" style="flex:1"></div>
        <div class="swatch" id="swatch"></div>
        <label class="field">Brilho <span id="brightText"></span></label>
        <input type="range" id="bright" min="5" max="100" value="100">
        <label class="field">Velocidade (respiração e arco-íris) <span id="speedText"></span></label>
        <input type="range" id="speed" min="1" max="10" step="0.5" value="5">
        <div class="row" style="margin-top:16px"><button class="btn" id="start" type="button">Ligar efeito</button><button class="btn outline" id="stop" type="button">Parar e devolver</button></div>
        <div class="live" id="live"></div>
        <label class="row sub" style="margin-top:14px"><input type="checkbox" id="autostart"> Aplicar minhas cores ao ligar o PC</label>
        <p class="sub" style="margin-top:6px">Ao fechar o app, os LEDs continuam como você deixou. O app lembra o efeito e reaplica quando abre.</p>
      </div>
    </div>
    <div class="grid wide" style="margin-top:12px">
      <div class="card"><h2>Detectado no seu PC</h2><div id="hw"><div class="sub">Procurando…</div></div></div>
      <div class="card"><h2>Preparar o sistema</h2><div id="env"><div class="sub">Verificando…</div></div></div>
    </div>`;

  const $ = (id) => el.querySelector("#" + id);
  let st = await invoke("light_status");
  let busy = false;

  // ---------------- desenho ----------------
  function paintStatus() {
    $("badge").textContent = st.running ? "efeito ligado" : st.connected ? "conectado" : "desconectado";
    if (!busy) $("msg").textContent = st.message;
    $("live").textContent = st.live;
    $("start").disabled = !st.connected || st.running;
    $("stop").disabled = !st.running;
    $("test").disabled = !st.connected || busy;
    $("ident").disabled = !st.connected || busy;
  }

  function paintControls() {
    const s = st.settings;
    el.querySelectorAll("#fx .chip").forEach((c) => c.classList.toggle("on", c.dataset.fx === s.effect));
    el.querySelectorAll("#palette .dotbtn").forEach((b) => b.classList.toggle("on", same(b.dataset.c.split(",").map(Number), s.color)));
    const { h, s: sat } = toHs(s.color);
    if (document.activeElement !== $("hue")) $("hue").value = h;
    if (document.activeElement !== $("sat")) $("sat").value = sat;
    $("colorText").textContent = `· RGB ${s.color.join(", ")}`;
    $("swatch").style.background = hex(s.color);
    $("bright").value = s.brightness;
    $("brightText").textContent = `· ${s.brightness}%`;
    $("speed").value = s.speed;
    $("speedText").textContent = `· ${String(s.speed).replace(".", ",")}`;
  }

  function paintDevices() {
    const s = st.settings;
    $("devs").innerHTML = st.devices.length
      ? st.devices.map((d) => `<div class="device"><input type="checkbox" data-id="${esc(d.id)}" ${s.selected.includes(d.id) ? "checked" : ""}>
          <div style="flex:1"><b>${esc(d.name)}</b> <span class="tag">${esc(d.kind)}</span><div class="sub">${esc(d.detail)}</div>
          ${d.zones.map((z, i) => `<div class="zone"><span class="zname">${esc(z.name)}${z.resizable ? ' <span class="faint">ARGB</span>' : ""}</span>
            ${z.resizable ? `<input type="number" min="${z.min}" max="${z.max}" value="${z.leds}" data-dev="${esc(d.id)}" data-zone="${i}" title="LEDs neste conector"><span class="faint">LEDs</span>` : `<span class="faint">${z.leds} LED${z.leds === 1 ? "" : "s"}</span>`}
            <button class="link" type="button" data-ident="${i}" data-dev="${esc(d.id)}" title="Acende só esta zona, em branco, por 4 s">identificar</button>
            <select data-dev="${esc(d.id)}" data-order="${i}" title="Ordem das cores">${ORDERS.map((o) => `<option value="${o}" ${o === z.order ? "selected" : ""}>${o.toUpperCase()}</option>`).join("")}</select>
            ${z.resizable && z.leds === 0 ? '<span class="warn-text">0 LEDs: o que estiver ligado aqui fica apagado</span>' : ""}</div>`).join("")}
          </div></div>`).join("")
      : `<div class="sub">${st.connected ? "O OpenRGB não encontrou dispositivos com LEDs." : "Os dispositivos aparecem aqui assim que o OpenRGB responder."}</div>`;
  }

  function paint() {
    paintStatus();
    paintControls();
    paintDevices();
  }

  // ---------------- envio ----------------
  // mudanças de cor/brilho chegam em rajada (arrastando o controle): manda só a última a cada 100 ms,
  // e liga o efeito sozinho, sem precisar do botão
  let pending = null;
  function push() {
    paintControls();
    clearTimeout(pending);
    pending = setTimeout(async () => {
      st = { ...(await invoke("light_apply", { settings: st.settings })), settings: st.settings };
      if (st.connected && !st.running) st = { ...(await invoke("light_start")), settings: st.settings };
      paintStatus();
    }, 100);
  }

  function setColor(c) {
    st.settings.color = c;
    if (!COLOR_EFFECTS.includes(st.settings.effect)) st.settings.effect = "static";
    push();
  }

  $("fx").addEventListener("click", (e) => {
    const b = e.target.closest(".chip");
    if (!b) return;
    st.settings.effect = b.dataset.fx;
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
    st.settings.brightness = +e.target.value;
    push();
  });
  $("speed").addEventListener("input", (e) => {
    st.settings.speed = +e.target.value;
    push();
  });

  $("devs").addEventListener("click", async (e) => {
    const b = e.target.closest("[data-ident]");
    if (!b) return;
    const d = st.devices.find((x) => x.id === b.dataset.dev);
    showBanner(`Acesa em <b>branco</b>: só <b>${esc(d?.name ?? "")} · ${esc(d?.zones[+b.dataset.ident]?.name ?? "")}</b>. Todos os LEDs dela acenderam?`);
    st = { ...(await invoke("light_identify", { device: b.dataset.dev, zone: +b.dataset.ident })), settings: st.settings };
    paintStatus();
  });

  function showBanner(html) {
    $("banner").hidden = false;
    $("banner").innerHTML = html;
  }

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

  $("devs").addEventListener("change", async (e) => {
    const t = e.target;
    if (t.dataset.zone != null) {
      const size = Math.max(+t.min, Math.min(+t.max, Math.round(+t.value || 0)));
      st = { ...(await invoke("light_resize", { device: t.dataset.dev, zone: +t.dataset.zone, size })), settings: st.settings };
      paint();
    } else if (t.dataset.order != null) {
      st = { ...(await invoke("light_order", { device: t.dataset.dev, zone: +t.dataset.order, order: t.value })), settings: st.settings };
      paint();
    } else if (t.dataset.id) {
      const sel = new Set(st.settings.selected);
      t.checked ? sel.add(t.dataset.id) : sel.delete(t.dataset.id);
      st.settings.selected = [...sel];
      push();
    }
  });

  // ---------------- teste de cores ----------------
  // Mostra vermelho, verde e azul puros. Se uma zona mostrar outra cor, a ordem dela está errada.
  $("test").addEventListener("click", async () => {
    busy = true;
    paintStatus();
    const keep = structuredClone(st.settings);
    const wasRunning = st.running;
    try {
      for (const [name, c] of [["VERMELHO", [255, 0, 0]], ["VERDE", [0, 255, 0]], ["AZUL", [0, 0, 255]]]) {
        showBanner(`Agora todos os LEDs deveriam estar <b style="color:${hex(c)}">${name}</b>`);
        await invoke("light_apply", { settings: { ...keep, effect: "static", color: c, brightness: 100 } });
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
      showBanner(`Não deu para mudar a opção de iniciar com o sistema: ${esc(String(err))}`);
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
