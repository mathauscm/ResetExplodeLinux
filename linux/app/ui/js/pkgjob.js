// Executa uma ação do pacman (atualizar, instalar, remover) com a janela de senha do sistema e mostra a saída
// ao vivo num painel. Usado pelas telas de Drivers e de Apps.

import { invoke, esc } from "./lib.js";

const listen = (ev, cb) => window.__TAURI__.event.listen(ev, cb);

/**
 * Roda `action` ({kind: "upgrade"|"install"|"remove", ...}) e escreve o andamento em `panel`.
 * Resolve com {ok, message} quando o pacman termina (ou quando nem chega a começar).
 */
export async function runJob(action, panel, title) {
  panel.hidden = false;
  panel.innerHTML = `<div class="row"><b>${esc(title)}</b><span class="spacer"></span><span class="tag" data-state>aguardando a senha…</span></div>
    <pre class="log" data-log></pre><div class="status" data-msg>Confirme a senha de administrador na janela do sistema.</div>`;
  const log = panel.querySelector("[data-log]");
  const state = panel.querySelector("[data-state]");
  const msg = panel.querySelector("[data-msg]");

  // os ouvintes entram antes de o trabalho começar, senão as primeiras linhas se perdem
  let done;
  const finished = new Promise((resolve) => (done = resolve));
  const unLog = await listen("pkg-log", (e) => {
    state.textContent = "em andamento";
    msg.textContent = "Não desligue o PC enquanto isso roda.";
    log.textContent += e.payload + "\n";
    log.scrollTop = log.scrollHeight;
  });
  const unDone = await listen("pkg-done", (e) => done(e.payload));

  try {
    await invoke("pkg_run", { action });
  } catch (e) {
    unLog();
    unDone();
    state.textContent = "não começou";
    msg.textContent = String(e);
    return { ok: false, message: String(e) };
  }
  const r = await finished;
  unLog();
  unDone();
  state.textContent = r.ok ? "concluído" : "falhou";
  state.classList.toggle("ok", r.ok);
  msg.textContent = r.message;
  return r;
}
