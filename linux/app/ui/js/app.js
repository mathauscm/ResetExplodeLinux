// Navegação, tema e o resumo da barra lateral. Cada página é um módulo em ./pages com
// `render(el)` (monta) e, se precisar, devolve uma função de limpeza (para timers).

import { snapshot, fmtTemp, fmtPct, bytes } from "./lib.js";
import * as dash from "./pages/dash.js";
import * as system from "./pages/system.js";
import * as sensors from "./pages/sensors.js";
import * as gpu from "./pages/gpu.js";
import * as disks from "./pages/disks.js";
import * as lights from "./pages/lights.js";
import * as drivers from "./pages/drivers.js";
import * as apps from "./pages/apps.js";
import * as energy from "./pages/energy.js";
import * as network from "./pages/network.js";
import * as library from "./pages/library.js";
import * as fps from "./pages/fps.js";
import * as bottleneck from "./pages/bottleneck.js";
import * as overlay from "./pages/overlay.js";
import * as bench from "./pages/bench.js";
import * as stress from "./pages/stress.js";
import * as oc from "./pages/oc.js";
import * as optimize from "./pages/optimize.js";
import * as startup from "./pages/startup.js";
import * as ports from "./pages/ports.js";
import * as serial from "./pages/serial.js";

const ICONS = {
  home: '<path d="M3 11l9-7 9 7v9a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>',
  cpu: '<rect x="6" y="6" width="12" height="12" rx="1"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>',
  pulse: '<path d="M2 12h4l3-8 4 16 3-8h6"/>',
  gpu: '<rect x="2" y="6" width="20" height="12" rx="1"/><circle cx="9" cy="12" r="3"/><path d="M15 10h4M15 14h4"/>',
  disk: '<ellipse cx="12" cy="6" rx="8" ry="3"/><path d="M4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>',
  bolt: '<path d="M13 2L4 14h7l-1 8 9-12h-7z"/>',
  net: '<path d="M2 9a15 15 0 0 1 20 0M5 13a10 10 0 0 1 14 0M8.5 16.5a5 5 0 0 1 7 0"/><circle cx="12" cy="20" r="1"/>',
  lib: '<rect x="3" y="4" width="18" height="14" rx="1"/><path d="M8 21h8M12 18v3"/>',
  fps: '<path d="M3 20h18M6 16V9M11 16V5M16 16v-4M21 16V8"/>',
  neck: '<path d="M4 4h16l-6 8v6l-4 2v-8z"/>',
  layers: '<path d="M12 3l9 5-9 5-9-5zM3 13l9 5 9-5"/>',
  gauge: '<path d="M4 18a8 8 0 1 1 16 0"/><path d="M12 18l4-6"/>',
  flame: '<path d="M12 22c4 0 7-3 7-7 0-5-5-7-5-12-3 2-4 5-4 7-1-1-2-2-2-4-2 2-3 5-3 9 0 4 3 7 7 7z"/>',
  oc: '<path d="M12 2v4M12 18v4M4.9 4.9l2.8 2.8M16.3 16.3l2.8 2.8M2 12h4M18 12h4M4.9 19.1l2.8-2.8M16.3 7.7l2.8-2.8"/><circle cx="12" cy="12" r="3"/>',
  tune: '<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0"/><circle cx="16" cy="6" r="2"/><circle cx="10" cy="12" r="2"/><circle cx="18" cy="18" r="2"/>',
  power: '<path d="M12 2v10"/><path d="M6.3 6.3a8 8 0 1 0 11.4 0"/>',
  box: '<path d="M3 7l9-4 9 4v10l-9 4-9-4z"/><path d="M3 7l9 4 9-4M12 11v10"/>',
  chip: '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M9 9h6v6H9z"/>',
  rgb: '<circle cx="9" cy="9" r="5"/><circle cx="15" cy="9" r="5"/><circle cx="12" cy="15" r="5"/>',
  plug: '<path d="M9 2v6M15 2v6M6 8h12v4a6 6 0 0 1-12 0zM12 18v4"/>',
  usb: '<path d="M12 2v16M8 6l4-4 4 4M7 11v3l5 3M17 9v3l-5 3"/><circle cx="12" cy="20" r="2"/>',
};

// Mesma estrutura do app do Windows.
const NAV = [
  { items: [{ id: "dash", label: "Visão geral", icon: "home", page: dash }] },
  { group: "RESET EXPLODE", items: [
    { id: "system", label: "Sistema", icon: "cpu", page: system },
    { id: "sensors", label: "Sensores", icon: "pulse", page: sensors },
    { id: "gpu", label: "Placa de vídeo", icon: "gpu", page: gpu },
    { id: "disks", label: "Discos", icon: "disk", page: disks },
    { id: "energy", label: "Energia", icon: "bolt", page: energy },
    { id: "net", label: "Rede", icon: "net", page: network },
  ] },
  { group: "RESET JOGOS", items: [
    { id: "library", label: "Biblioteca", icon: "lib", page: library },
    { id: "fps", label: "FPS estimado", icon: "fps", page: fps },
    { id: "bottleneck", label: "Gargalo", icon: "neck", page: bottleneck },
    { id: "overlay", label: "Overlay", icon: "layers", page: overlay },
  ] },
  { group: "RESET DESEMPENHO", items: [
    { id: "bench", label: "Benchmark", icon: "gauge", page: bench },
    { id: "stress", label: "Stress test", icon: "flame", page: stress },
    { id: "oc", label: "Overclock", icon: "oc", page: oc },
  ] },
  { group: "RESET FERRAMENTAS", items: [
    { id: "lights", label: "Iluminação RGB", icon: "rgb", page: lights },
    { id: "optimize", label: "Otimização", icon: "tune", page: optimize },
    { id: "startup", label: "Inicialização", icon: "power", page: startup },
    { id: "apps", label: "Apps", icon: "box", page: apps },
    { id: "drivers", label: "Drivers e atualizações", icon: "chip", page: drivers },
  ] },
  { group: "RESET MAKER", items: [
    { id: "ports", label: "Portas seriais", icon: "plug", page: ports },
    { id: "serial", label: "Drivers seriais", icon: "usb", page: serial },
  ] },
];

const svg = (name) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] ?? ""}</svg>`;

const nav = document.getElementById("nav");
const pageEl = document.getElementById("page");
let cleanup = null;
let navToken = 0;

function buildNav() {
  const closed = JSON.parse(store("navClosed") ?? "[]");
  nav.innerHTML = NAV.map((g) => {
    const items = g.items
      .map((it) => `<button class="nav-item" data-id="${it.id}" type="button">${svg(it.icon)}<span>${it.label}</span></button>`)
      .join("");
    if (!g.group) return `<div class="nav-items">${items}</div>`;
    const cls = closed.includes(g.group) ? " closed" : "";
    return `<button class="nav-group${cls}" type="button" data-group="${g.group}">${g.group}<span class="chev">▾</span></button><div class="nav-items">${items}</div>`;
  }).join("");

  nav.addEventListener("click", (e) => {
    const grp = e.target.closest(".nav-group");
    if (grp) {
      grp.classList.toggle("closed");
      const now = [...nav.querySelectorAll(".nav-group.closed")].map((b) => b.dataset.group);
      store("navClosed", JSON.stringify(now));
      return;
    }
    const item = e.target.closest(".nav-item");
    if (item) go(item.dataset.id);
  });
}

function go(id) {
  const item = NAV.flatMap((g) => g.items).find((i) => i.id === id) ?? NAV[0].items[0];
  nav.querySelectorAll(".nav-item").forEach((b) => b.classList.toggle("active", b.dataset.id === item.id));
  if (typeof cleanup === "function") cleanup();
  cleanup = null;
  pageEl.scrollTop = 0;
  // cada visita ganha um contêiner novo: uma página lenta que termine depois da troca escreve num nó solto
  const box = document.createElement("div");
  pageEl.replaceChildren(box);
  const token = ++navToken;
  const mod = item.page;
  Promise.resolve(mod.render(box, item)).then((fn) => {
    if (token === navToken) cleanup = fn;
    else if (typeof fn === "function") fn();
  });
  store("page", item.id);
}

// ---------------- tema ----------------
const themeBtn = document.getElementById("themeBtn");
function applyTheme(light) {
  document.documentElement.dataset.theme = light ? "light" : "dark";
  themeBtn.textContent = light ? "tema escuro" : "tema claro";
  // logo.png é clara (para fundo escuro); logo-dark.png é escura
  document.querySelectorAll("[data-logo]").forEach((img) => (img.src = light ? "assets/logo-dark.png" : "assets/logo.png"));
  store("theme", light ? "light" : "dark");
}
themeBtn.addEventListener("click", () => applyTheme(document.documentElement.dataset.theme !== "light"));

// ---------------- resumo da barra lateral (a cada 2 s) ----------------
const miniCpu = document.getElementById("miniCpu");
const miniGpu = document.getElementById("miniGpu");
const miniRam = document.getElementById("miniRam");
async function mini() {
  try {
    const { summary: s } = await snapshot();
    miniCpu.textContent = `${fmtTemp(s.cpu_temp_c)} · ${fmtPct(s.cpu_load_pct)}`;
    miniGpu.textContent = s.gpu ? `${fmtTemp(s.gpu.temp_c)} · ${fmtPct(s.gpu.load_pct)}` : "—";
    miniRam.textContent = `${bytes(s.ram_used)} / ${bytes(s.ram_total)}`;
  } catch {
    /* backend ainda subindo */
  }
}

function store(k, v) {
  try {
    if (v === undefined) return localStorage.getItem("re." + k);
    localStorage.setItem("re." + k, v);
  } catch {
    return null;
  }
}

buildNav();
applyTheme(store("theme") === "light");
go(store("page") ?? "dash");
mini();
setInterval(mini, 2000);
