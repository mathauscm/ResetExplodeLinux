// Drivers seriais: no Linux os drivers de CH340, CP210x, FTDI, PL2303 e CDC-ACM vêm no kernel, então não há
// o que baixar. A tela mostra cada módulo, o diagnóstico das placas conectadas e corrige o que atrapalha de
// verdade: o brltty capturando conversores, a regra udev (acesso e ModemManager) e a Arduino IDE.

import { call, head, esc } from "../lib.js";
import { runJob } from "../pkgjob.js";

const dot = (s) => `<span class="dot ${s}"></span>`;
const CONVERTERS = ["ch340", "ch9102", "cp210x", "ftdi", "pl2303"];

export async function render(el) {
  el.innerHTML = head("RESET MAKER", "Drivers seriais", `<button class="btn outline" id="scan" type="button">Verificar de novo</button>`) + `
    <p class="sub sd-intro">No Linux, os drivers de <b class="fg">CH340, CP210x, FTDI e PL2303</b> e o <b class="fg">CDC-ACM</b> (Arduino Uno/Leonardo, ESP32-S2/S3/C3, Pico) já vêm no kernel: não existe driver para baixar nem versão para trocar. Quando a placa não aparece, a causa costuma estar em volta: o brltty, permissão ou o ModemManager. É isso que esta tela verifica.</p>
    <div class="card job" id="job" hidden style="margin-bottom:12px"></div>
    <div id="body"><div class="card"><div class="sub">Verificando placas, módulos do kernel e regras…</div></div></div>`;

  const $ = (id) => el.querySelector("#" + id);
  let r = null;
  let busy = false;
  let confirm = null; // ação esperando o segundo clique

  // ---------------------------------------------------------------- blocos
  function devices() {
    if (!r.devices.length)
      return `<div class="sd-empty">${dot("")}<div><b>Nenhuma placa ou conversor serial conectado</b>
        <div class="sub">Conecte a placa e clique em <b>Verificar de novo</b>. Se ela não aparecer nem aqui, troque o cabo: muitos são só de carga.</div></div></div>`;
    return r.devices.map((d) => {
      // conversor (CH340, CP2102...) vai em muitas placas: o título é o chip e a placa vira "provável"
      const conv = CONVERTERS.includes(d.chip_id);
      const name = (conv ? d.chip : d.board) ?? d.product ?? d.chip ?? "Dispositivo USB";
      const detail = [conv ? null : d.chip, `VID ${d.vid} · PID ${d.pid}`, `USB ${d.usb}`, d.iface_drivers.length ? `driver ${d.iface_drivers.join(", ")}` : "sem driver"].filter(Boolean);
      const mod = r.modules.find((m) => m.module === d.module);
      const canLoad = d.state !== "ok" && mod && mod.status.file && !mod.status.loaded;
      const brl = /brltty/.test(d.verdict);
      return `<div class="sd-dev"><div class="sd-dev-top">${dot(d.state)}
          <div style="flex:1;min-width:0"><b>${esc(name)}</b>${conv && d.board ? `<div class="sub sd-board">Placa provável: ${esc(d.board)}</div>` : ""}<div class="faint mono">${esc(detail.join(" · "))}</div></div>
          <span class="sd-state ${d.state}">${d.state === "ok" ? "funcionando" : "com problema"}</span></div>
        <div class="sd-verdict">${esc(d.verdict)}</div>
        ${d.advice ? `<div class="sub">${esc(d.advice)}</div>` : ""}
        ${canLoad || brl ? `<div class="row" style="margin-top:8px">
          ${canLoad ? `<button class="btn outline sd-small" type="button" data-modprobe="${esc(d.module)}" ${busy ? "disabled" : ""}>Carregar módulo ${esc(d.module)}</button>` : ""}
          ${brl ? `<button class="btn outline sd-small" type="button" data-scroll="brl">Ver a correção</button>` : ""}</div>` : ""}
      </div>`;
    }).join("");
  }

  function modules() {
    return `<table class="sd-mods"><thead><tr><th>MÓDULO</th><th>CHIPS</th><th>NO KERNEL</th><th>AGORA</th></tr></thead><tbody>${r.modules
      .map((m) => {
        const s = m.status;
        const avail = s.builtin ? `<span class="sd-ok">embutido</span>` : s.file ? `<span class="sd-ok">disponível</span>` : `<span class="sd-bad">ausente</span>`;
        const now = m.ports.length ? `<b class="fg">em uso</b> <span class="faint mono">${esc(m.ports.join(" "))}</span>` : s.loaded ? "carregado" : `<span class="faint">carrega ao conectar</span>`;
        return `<tr><td class="mono fg">${esc(m.module)}</td><td class="sd-chips">${esc(m.chips.map((c) => c.replace(/ \((USB nativo|USB CDC)\)/, "")).join(", "))}</td><td>${avail}</td><td>${now}</td></tr>`;
      })
      .join("")}</tbody></table>
      <p class="faint small" style="margin:10px 0 0">Kernel ${esc(r.kernel)} · arquivos em <code>/usr/lib/modules/${esc(r.kernel)}</code>. O sistema carrega o módulo sozinho quando a placa conecta.</p>`;
  }

  function brltty() {
    const b = r.brltty;
    const catching = b.rules.filter((x) => x.catches_ch340 && !x.masked);
    const masked = b.rules.filter((x) => x.catches_ch340 && x.masked);
    let html = "";
    if (!b.installed && !catching.length) {
      html += `<div class="check">${dot("ok")}<div><b>brltty não instalado</b><div class="sub">Nada vai capturar seus conversores CH340/CP210x.</div></div></div>`;
    } else if (!catching.length) {
      html += `<div class="check">${dot("ok")}<div><b>brltty ${esc(b.installed ?? "")} instalado, sem regra para conversores genéricos</b>
        <div class="sub">${masked.length ? "A regra que pegava o CH340 está desativada." : "No Arch, a regra que captura CH340/CP210x fica no pacote à parte <code>brltty-udev-generic</code>. Se você usa Arduino ou ESP, não instale esse pacote."}</div></div></div>`;
    } else {
      const owner = b.remove;
      const blocked = b.required_by.length > 0;
      html += `<div class="check">${dot("bad")}<div style="flex:1"><b>O brltty vai capturar conversores CH340</b>
        <div class="sub">A regra ${catching.map((x) => `<code>${esc(x.name)}</code>`).join(", ")}${owner ? ` (pacote <code>${esc(owner)}</code>)` : ""} diz que conversores USB-serial genéricos são linhas braille. Quando o CH340 conecta, o brltty prende o dispositivo e a porta <code>/dev/ttyUSB0</code> some segundos depois. É o problema mais comum de Arduino no Arch.${b.running ? " O brltty está rodando agora." : ""}</div>
        <div class="sd-actions">
          ${owner ? `<button class="btn ${confirm === "rm-brl" ? "danger" : ""}" type="button" data-act="rm-brl" ${busy || blocked || r.pacman_locked ? "disabled" : ""}>${confirm === "rm-brl" ? "Confirmar remoção" : `Remover ${esc(owner)}`}</button>` : ""}
          <button class="btn outline" type="button" data-act="mask" ${busy ? "disabled" : ""}>Desativar só a regra</button>
        </div>
        <div class="faint small">${blocked ? `Não dá para remover: ${esc(b.required_by.join(", "))} depende(m) dele. Desative só a regra. ` : ""}Usa uma linha braille? Prefira <b>desativar só a regra</b>: o brltty continua funcionando com o modelo configurado. Depois, desconecte e conecte o conversor.</div>
      </div></div>`;
    }
    if (masked.length)
      html += `<div class="check">${dot("ok")}<div style="flex:1"><b>Regra desativada</b> <span class="faint mono">${masked.map((x) => esc(x.name)).join(", ")}</span>
        <div class="sub">Um link para <code>/dev/null</code> em <code>/etc/udev/rules.d</code> anula a regra original.</div></div>
        <button class="link" type="button" data-act="unmask" ${busy ? "disabled" : ""}>reativar</button></div>`;
    return html;
  }

  function udev() {
    const u = r.udev;
    const status = u.installed
      ? u.outdated
        ? `<div class="check">${dot("warn")}<div><b>Regra instalada, de uma versão anterior</b><div class="sub">Atualize para cobrir as placas novas da lista.</div></div></div>`
        : `<div class="check">${dot("ok")}<div><b>Regra do RESET MAKER instalada</b><div class="sub mono">${esc(u.path)}</div></div></div>`
      : `<div class="check">${dot("")}<div><b>Regra do RESET MAKER não instalada</b><div class="sub">Opcional, mas resolve dois problemas de uma vez.</div></div></div>`;
    const mm = u.modemmanager
      ? `<div class="check">${dot(u.installed ? "ok" : "warn")}<div><b>ModemManager rodando</b><div class="sub">${u.installed ? "A regra já faz ele ignorar as placas." : "Ele testa placas <code>ttyACM</code> como se fossem modem nos primeiros segundos e o upload pode falhar. A regra resolve."}</div></div></div>`
      : "";
    const pio = u.platformio
      ? `<div class="check">${dot("ok")}<div><b>Regras do PlatformIO instaladas</b> <span class="faint mono">platformio-core-udev ${esc(u.platformio)}</span></div></div>`
      : u.platformio_repo
        ? `<div class="check">${dot("")}<div style="flex:1"><b>Usa PlatformIO?</b><div class="sub">O pacote oficial <code>platformio-core-udev</code> (${esc(u.platformio_repo)}) traz as regras de centenas de placas e gravadores.</div></div>
           <button class="btn outline sd-small" type="button" data-act="pio" ${busy || r.pacman_locked ? "disabled" : ""}>Instalar</button></div>`
        : "";
    return `${status}
      <p class="sub" style="margin:8px 0">A regra faz duas coisas para Arduino, ESP32/ESP8266, Pico, STM32 e conversores CH340, CP210x, FTDI e PL2303:
        dá acesso à porta para quem está logado (<code>uaccess</code>, vale na hora, sem grupo nem sair da sessão) e manda o ModemManager deixar a placa em paz.</p>
      <details class="sd-rule"><summary>O que será gravado em <code>${esc(u.path)}</code></summary><pre>${esc(u.content)}</pre></details>
      <div class="sd-actions">
        <button class="btn" type="button" data-act="udev" ${busy || (u.installed && !u.outdated) ? "disabled" : ""}>${u.installed ? (u.outdated ? "Atualizar regra" : "Regra em dia") : "Instalar regra"}</button>
        ${u.installed ? `<button class="btn ${confirm === "udev-rm" ? "danger" : "outline"}" type="button" data-act="udev-rm" ${busy ? "disabled" : ""}>${confirm === "udev-rm" ? "Confirmar" : "Remover regra"}</button>` : ""}
        <span class="faint small">Pede a senha. Depois, desconecte e conecte a placa.</span>
      </div>
      ${mm}${pio}`;
  }

  function ide() {
    const i = r.ide;
    const inRepo = ["arduino-ide", "arduino-ide-bin"].find((n) => i.repo[n]);
    const cliRepo = i.repo["arduino-cli"];
    let html = i.installs.length
      ? i.installs.map((x) => `<div class="check">${dot("ok")}<div><b>${esc(x.name)}</b>${x.version ? ` <span class="faint mono">${esc(x.version)}</span>` : ""} <span class="tag">${esc(x.kind)}</span>
          ${x.path ? `<div class="faint mono ellipsis">${esc(x.path)}</div>` : ""}</div></div>`).join("")
      : `<div class="check">${dot("")}<div><b>Arduino IDE não encontrada</b><div class="sub">Procurei pacotes, Flatpak e AppImage na pasta pessoal.</div></div></div>`;
    if (i.running) html += `<div class="check">${dot("warn")}<div><b>A IDE está aberta agora</b><div class="sub">O monitor serial dela também prende a porta: feche-o antes de usar outro programa na mesma placa.</div></div></div>`;

    const opts = [];
    if (!i.installs.length) {
      if (inRepo)
        opts.push(`<div class="sd-opt"><div style="flex:1"><b>Pelos repositórios configurados</b><div class="sub">O pacote <code>${esc(inRepo)}</code> está em <code>${esc(i.repo[inRepo][0])}</code> (${esc(i.repo[inRepo][1])}).</div></div>
          <button class="btn" type="button" data-act="ide-pkg" data-pkg="${esc(inRepo)}" ${busy || r.pacman_locked ? "disabled" : ""}>Instalar</button></div>`);
      if (i.flathub)
        opts.push(`<div class="sd-opt"><div style="flex:1"><b>Pelo Flathub</b><div class="sub">Pacote oficial <code>cc.arduino.IDE2</code>, ${i.flathub === "user" ? "só para o seu usuário (sem senha)" : "para o sistema"}.</div></div>
          <button class="btn ${inRepo ? "outline" : ""}" type="button" data-act="ide-flatpak" ${busy ? "disabled" : ""}>Instalar</button></div>`);
      if (!inRepo) {
        const cmd = `${i.aur_helper ?? "paru"} -S arduino-ide-bin`;
        opts.push(`<div class="sd-opt"><div style="flex:1"><b>Pelo AUR</b><div class="sub">A Arduino IDE 2 <b>não está nos repositórios oficiais</b> do Arch: só no AUR (pacotes mantidos pela comunidade) e no Flathub. O app não instala do AUR sozinho; rode no terminal:</div>
          <div class="sd-cmd"><code>${esc(cmd)}</code><button class="link" type="button" data-copy="${esc(cmd)}">copiar</button></div>
          ${i.aur_helper ? "" : `<div class="faint small">Precisa de um ajudante do AUR (paru ou yay). Sem ele, baixe o AppImage em arduino.cc/en/software.</div>`}</div></div>`);
      }
    }
    if (!i.cli && cliRepo)
      opts.push(`<div class="sd-opt"><div style="flex:1"><b>arduino-cli</b> <span class="faint mono">${esc(cliRepo[1])}</span><div class="sub">Compila e envia pelo terminal (usado por VS Code e outros editores). Está nos repositórios oficiais.</div></div>
        <button class="btn outline" type="button" data-act="cli" ${busy || r.pacman_locked ? "disabled" : ""}>Instalar</button></div>`);
    else if (i.cli) html += `<div class="check">${dot("ok")}<div><b>arduino-cli</b> <span class="faint mono">${esc(i.cli)}</span></div></div>`;
    return html + (opts.length ? `<div class="sd-opts">${opts.join("")}</div>` : "") +
      `<p class="faint small" style="margin:12px 0 0">Na IDE, a placa aparece como <code>/dev/ttyUSB0</code> (conversor) ou <code>/dev/ttyACM0</code> (USB nativo). Erro <i>Permission denied</i>: veja o grupo em <b>Portas seriais</b> ou instale a regra acima.
        Instalar pelo pacman também aplica as atualizações pendentes do sistema (o Arch não suporta instalar com a lista desatualizada).</p>`;
  }

  function draw() {
    const bad = r.devices.filter((d) => d.state !== "ok").length;
    const summary = !r.devices.length ? "Nenhuma placa conectada" : bad ? `${bad} de ${r.devices.length} com problema` : `${r.devices.length} conectada(s), tudo funcionando`;
    $("body").innerHTML = `
      ${r.pacman_locked ? `<div class="test-banner" style="margin:0 0 12px">Outro programa está instalando ou atualizando pacotes agora. Os botões de pacote voltam quando ele terminar.</div>` : ""}
      <div class="grid wide">
        <div class="card"><div class="row"><h2 style="margin:0">Placas e conversores conectados</h2><span class="spacer"></span><span class="faint small">${esc(summary)}</span></div>
          <div class="sd-devs">${devices()}</div></div>
        <div class="card"><h2>Drivers no kernel</h2>${modules()}</div>
      </div>
      <div class="grid wide" style="margin-top:12px">
        <div class="card" id="brl"><h2>brltty (leitor de braille)</h2>${brltty()}</div>
        <div class="card"><h2>Regra udev: acesso e ModemManager</h2>${udev()}</div>
      </div>
      <div class="card" style="margin-top:12px"><h2>Arduino IDE</h2>${ide()}</div>`;
  }

  async function load() {
    $("scan").disabled = true;
    try {
      r = await call("maker", "drivers");
      draw();
    } catch (e) {
      $("body").innerHTML = `<div class="card"><div class="check">${dot("bad")}<div><b>Não deu para verificar</b><div class="sub">${esc(String(e))}</div></div></div></div>`;
    } finally {
      $("scan").disabled = busy;
    }
  }

  async function job(start, title) {
    busy = true;
    confirm = null;
    draw();
    const res = await runJob(start, $("job"), title);
    busy = false;
    await load();
    return res;
  }

  el.addEventListener("click", async (e) => {
    const t = e.target.closest("button");
    if (!t || t.disabled) return;
    if (t.id === "scan") return load();
    if (t.dataset.copy) {
      try {
        await navigator.clipboard.writeText(t.dataset.copy);
        t.textContent = "copiado";
      } catch {
        t.textContent = "selecione e copie";
      }
      setTimeout(() => (t.textContent = "copiar"), 1500);
      return;
    }
    if (t.dataset.scroll) return $(t.dataset.scroll)?.scrollIntoView({ behavior: "smooth", block: "start" });
    if (t.dataset.modprobe) return job(() => call("maker", "modprobe", { module: t.dataset.modprobe }), `Carregando o módulo ${t.dataset.modprobe}`);
    const act = t.dataset.act;
    if (!act) return;
    // ações que removem algo pedem um segundo clique (sem diálogo do navegador, que o Tauri bloqueia)
    if ((act === "rm-brl" || act === "udev-rm") && confirm !== act) {
      confirm = act;
      draw();
      return;
    }
    switch (act) {
      case "rm-brl":
        return job({ kind: "remove", packages: [r.brltty.remove] }, `Removendo ${r.brltty.remove}`);
      case "mask":
        return job(() => call("maker", "brltty_mask", { unmask: false }), "Desativando a regra do brltty");
      case "unmask":
        return job(() => call("maker", "brltty_mask", { unmask: true }), "Reativando a regra do brltty");
      case "udev":
        return job(() => call("maker", "udev_install"), "Instalando a regra udev do RESET MAKER");
      case "udev-rm":
        return job(() => call("maker", "udev_remove"), "Removendo a regra udev do RESET MAKER");
      case "pio":
        return job({ kind: "install", packages: ["platformio-core-udev"] }, "Instalando as regras do PlatformIO");
      case "cli":
        return job({ kind: "install", packages: ["arduino-cli"] }, "Instalando o arduino-cli");
      case "ide-pkg":
        return job({ kind: "install", packages: [t.dataset.pkg] }, "Instalando a Arduino IDE");
      case "ide-flatpak":
        return job(() => call("maker", "ide_flatpak"), "Instalando a Arduino IDE pelo Flathub");
    }
  });

  await load();
}
