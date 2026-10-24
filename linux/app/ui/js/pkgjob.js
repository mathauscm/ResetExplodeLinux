// Executa uma tarefa longa (pacman, ajustes de sistema...) e mostra a saída ao vivo num painel. Tarefas com
// root pedem a senha pela janela do sistema. Usado pelas telas de Drivers, Apps, Otimização, Overclock...

import { invoke, esc, listen } from "./lib.js";

/**
 * `start` é uma ação do pacman ({kind: "upgrade"|"install"|"remove", ...}) ou uma função que inicia a
 * tarefa no backend (ex.: `() => call("tools", "apply", {...})`, que por baixo usa `jobs::spawn`).
 * Escreve o andamento em `panel` e resolve com {ok, message} quando a tarefa termina.
 */
export async function runJob(start, panel, title) {
  panel.hidden = false;
  panel.innerHTML = `<div class="row"><b>${esc(title)}</b><span class="spacer"></span><span class="tag" data-state>começando…</span></div>
    <pre class="log" data-log></pre><div class="status" data-msg>Se pedir, confirme a senha de administrador na janela do sistema.</div>`;
  const log = panel.querySelector("[data-log]");
  const state = panel.querySelector("[data-state]");
  const msg = panel.querySelector("[data-msg]");

  // os ouvintes entram antes de a tarefa começar, senão as primeiras linhas se perdem
  let done;
  const finished = new Promise((resolve) => (done = resolve));
  const unLog = await listen("job-log", (line) => {
    state.textContent = "em andamento";
    msg.textContent = "Não desligue o PC enquanto isso roda.";
    log.textContent += line + "\n";
    log.scrollTop = log.scrollHeight;
  });
  const unDone = await listen("job-done", (r) => done(r));

  try {
    await (typeof start === "function" ? start() : invoke("pkg_run", { action: start }));
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
