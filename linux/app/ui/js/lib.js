// Utilidades comuns às páginas: chamada ao backend e formatação no padrão brasileiro.

export const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);

// Uma leitura de sensores serve a página e a barra lateral ao mesmo tempo: a carga da CPU é calculada
// entre duas leituras, e leituras muito próximas dariam valores ruidosos.
let lastSnap = { at: 0, p: null };
export function snapshot() {
  if (lastSnap.p && Date.now() - lastSnap.at < 800) return lastSnap.p;
  const p = invoke("sensors_poll");
  lastSnap = { at: Date.now(), p };
  p.catch(() => (lastSnap = { at: 0, p: null }));
  return p;
}

const nf = (d) => new Intl.NumberFormat("pt-BR", { minimumFractionDigits: d, maximumFractionDigits: d });
export const num = (v, d = 0) => (v == null || Number.isNaN(v) ? "—" : nf(d).format(v));
export const fmtTemp = (v) => (v == null ? "—" : `${num(v)} °C`);
export const fmtPct = (v) => (v == null ? "—" : `${num(v)}%`);
export const fmtMhz = (v) => (v == null ? "—" : `${num(v)} MHz`);
export const fmtW = (v) => (v == null ? "—" : `${num(v, 1)} W`);

export function bytes(b, d = 1) {
  if (b == null) return "—";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (b >= 1024 && i < u.length - 1) {
    b /= 1024;
    i++;
  }
  return `${num(b, i === 0 || b >= 100 ? 0 : d)} ${u[i]}`;
}

export function duration(s) {
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return [d && `${d} d`, h && `${h} h`, `${m} min`].filter(Boolean).join(" ");
}

/** Escapa texto vindo do hardware (nomes de dispositivos) antes de pôr no HTML. */
export function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

export function head(eyebrow, title, right = "") {
  return `<div class="page-head"><div><div class="eyebrow">${eyebrow}</div><h1>${title}</h1></div><div class="row">${right}</div></div>`;
}

/** Lista chave/valor; linhas com valor vazio somem. */
export function kv(rows) {
  return `<dl class="kv">${rows
    .filter(([, v]) => v != null && v !== "")
    .map(([k, v]) => `<dt>${esc(k)}</dt><dd>${esc(v)}</dd>`)
    .join("")}</dl>`;
}

export function bar(pct, hot = false) {
  const p = Math.max(0, Math.min(100, pct ?? 0));
  return `<div class="bar${hot ? " hot" : ""}"><i style="width:${p}%"></i></div>`;
}

/** Repete `fn` a cada `ms` até a página sair; devolve a função de limpeza. */
export function every(ms, fn) {
  let alive = true;
  const tick = async () => {
    if (!alive) return;
    try {
      await fn();
    } catch (e) {
      console.error(e);
    }
    if (alive) timer = setTimeout(tick, ms);
  };
  let timer = setTimeout(tick, 0);
  return () => {
    alive = false;
    clearTimeout(timer);
  };
}
