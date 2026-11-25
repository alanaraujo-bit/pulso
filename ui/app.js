"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (s, el = document) => el.querySelector(s);
const panel = $("#panel");
const list = $("#list");
const settingsView = $("#settings");
const tpl = $("#tpl-device");

let devices = [];
let settings = {};
let skew = 0; // backend clock minus local clock, seconds
let uid = 0;

const GLYPHS = {
  mouse: '<rect x="18" y="13.5" width="12" height="21" rx="6"/><path d="M24 13.5v6.5"/>',
  headset: '<path d="M14.5 28v-3.5a9.5 9.5 0 0 1 19 0V28"/><path d="M14.5 27h2.8a1.7 1.7 0 0 1 1.7 1.7v4.1a1.7 1.7 0 0 1-1.7 1.7h-1.6a1.2 1.2 0 0 1-1.2-1.2z"/><path d="M33.5 27h-2.8a1.7 1.7 0 0 0-1.7 1.7v4.1a1.7 1.7 0 0 0 1.7 1.7h1.6a1.2 1.2 0 0 0 1.2-1.2z"/>',
  keyboard: '<rect x="12.5" y="17.5" width="23" height="13" rx="2.6"/><path d="M16.5 21.5h1M20.5 21.5h1M24.5 21.5h1M28.5 21.5h1M31 21.5h.5M17 25h1M21 25h1M25 25h1M29 25h2M19.5 27.5h9"/>',
  other: '<circle cx="24" cy="24" r="6.5"/>',
};

const nowSec = () => Date.now() / 1000 + skew;

// ---------- formatting (pt-BR, compact) ----------

function dur(secs) {
  const m = Math.max(0, Math.round(secs / 60));
  if (m < 1) return "<1 min";
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 10) return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  if (h < 48) return `${h} h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

function ago(ts) {
  const s = nowSec() - ts;
  if (s < 90) return "há instantes";
  return `há ${dur(s)}`;
}

function stateOf(d) {
  if (d.charge === "charging") return "charging";
  if (d.charge === "full") return "full";
  if (d.percent != null && d.percent <= 15) return "low";
  if (d.percent != null && d.percent <= 25) return "warn";
  return "normal";
}

function statusText(d) {
  if (!d.online) {
    return d.percent == null || !d.last_seen
      ? "Aguardando acordar · mexa ou aperte uma tecla"
      : `Em repouso · visto ${ago(d.last_seen)}`;
  }
  switch (d.charge) {
    case "charging": return "Carregando";
    case "full": return "Carga completa";
    default: return d.percent != null && d.percent <= 15 ? "Bateria baixa" : "Em uso";
  }
}

function stats(d) {
  if (!d.online) return [];
  const now = nowSec();
  const out = [];
  if (d.charge === "charging" && d.to_full != null) {
    out.push(["Cheio em", `~${dur(d.to_full)}`]);
  } else if (d.charge === "discharging" && d.remaining != null) {
    const tip = d.confidence < 0.05
      ? `Pela autonomia oficial (${d.rated_hours} h). Fica mais preciso conforme o uso é medido.`
      : `${Math.round(d.confidence * 100)}% baseado no seu uso medido.`;
    out.push(["Restante", `~${dur(d.remaining)}`, tip]);
  }
  if (d.session_start != null) {
    out.push(d.session_observed
      ? ["Ligado há", dur(now - d.session_start)]
      : ["Ativo há", dur(now - d.session_start), "Contando desde que o Pulso o encontrou ligado"]);
  }
  if (d.charge === "discharging" && d.unplugged_at != null) {
    out.push(["Desde a carga", dur(now - d.unplugged_at)]);
  }
  return out;
}

// ---------- rendering ----------

function animateNumber(el, to) {
  const from = Number(el.dataset.v ?? to);
  el.dataset.v = to;
  if (from === to || to == null) { el.textContent = to ?? "–"; return; }
  const t0 = performance.now();
  const step = (t) => {
    const k = Math.min(1, (t - t0) / 650);
    const e = 1 - Math.pow(1 - k, 3);
    el.textContent = Math.round(from + (to - from) * e);
    if (k < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

function card(d) {
  let el = list.querySelector(`[data-id="${CSS.escape(d.id)}"]`);
  if (!el) {
    el = tpl.content.firstElementChild.cloneNode(true);
    el.dataset.id = d.id;
    const id = `clip${++uid}`;
    const svg = $(".orb svg", el);
    const defs = document.createElementNS("http://www.w3.org/2000/svg", "defs");
    defs.innerHTML = `<clipPath id="${id}"><circle cx="24" cy="24" r="19"/></clipPath>`;
    svg.prepend(defs);
    $(".water", el).setAttribute("clip-path", `url(#${id})`);
    $(".glyph", el).innerHTML = GLYPHS[d.kind] || GLYPHS.other;
    // Start empty so the first paint fills up.
    $(".lvl", el).style.transform = "translateY(44px)";
  }
  el.dataset.state = stateOf(d);
  el.classList.toggle("offline", !d.online);
  $(".name", el).textContent = d.name;
  $(".name", el).title = d.name;
  const st = $(".status", el);
  st.innerHTML = '<span class="dot"></span>';
  st.append(statusText(d));

  const pct = d.percent;
  animateNumber($(".num", el), pct);
  // Water surface spans the inner bowl (y 5..43); the wave crest sits at path y=4.
  const y = pct == null ? 44 : 5 + 38 * (1 - pct / 100) - 4;
  requestAnimationFrame(() => {
    $(".lvl", el).style.transform = `translateY(${y.toFixed(2)}px)`;
    $(".ring", el).style.strokeDasharray = `${pct ?? 0} 100`;
  });

  const dl = $(".stats", el);
  const rows = stats(d);
  dl.style.gridTemplateColumns = `repeat(${Math.max(rows.length, 1)}, 1fr)`;
  dl.innerHTML = "";
  for (const [k, v, tip] of rows) {
    const div = document.createElement("div");
    if (tip) div.title = tip;
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    div.append(dt, dd);
    dl.append(div);
  }
  el.setAttribute("aria-label", `${d.name}: ${pct ?? "sem leitura"}%, ${statusText(d)}`);
  return el;
}

function render() {
  if (!devices.length) {
    list.innerHTML = '<div class="empty"><div class="ring"></div><b>Procurando dispositivos</b>Ligue ou mexa no periférico para acordá-lo.</div>';
    $("#summary").textContent = "";
    return;
  }
  list.querySelector(".empty")?.remove();
  const keep = new Set();
  devices.forEach((d, i) => {
    const el = card(d);
    el.style.setProperty("--i", i);
    keep.add(el);
    if (list.children[i] !== el) list.insertBefore(el, list.children[i] || null);
  });
  [...list.children].forEach((c) => keep.has(c) || c.remove());

  const on = devices.filter((d) => d.online).length;
  $("#summary").textContent = on === devices.length
    ? `${on} ${on === 1 ? "conectado" : "conectados"}`
    : `${on} de ${devices.length} conectados`;
}

function renderSettings() {
  for (const input of settingsView.querySelectorAll("input[data-key]")) {
    const k = input.dataset.key;
    input.checked = k === "mode" ? settings.mode === "floating" : !!settings[k];
  }
  $("#row-ontop").classList.toggle("disabled", settings.mode !== "floating");
}

// ---------- window fit & motion ----------

function fit() {
  // Layout box, not getBoundingClientRect: the enter animation scales the panel.
  const views = $(".views");
  const h = Math.ceil(views.offsetTop + views.offsetHeight + 12);
  invoke("fit", { height: h }).catch(() => {});
}
new ResizeObserver(fit).observe($(".views"));

function enter() {
  panel.classList.remove("enter", "leave");
  void panel.offsetWidth;
  panel.classList.add("enter");
}

let leaving = false;
function dismiss() {
  if (leaving) return;
  leaving = true;
  panel.classList.remove("enter");
  panel.classList.add("leave");
  setTimeout(() => {
    invoke("hide").finally(() => {
      panel.classList.remove("leave");
      leaving = false;
    });
  }, 130);
}

// ---------- wiring ----------

$("#btn-refresh").addEventListener("click", (e) => {
  const b = e.currentTarget;
  b.classList.remove("spin");
  void b.offsetWidth;
  b.classList.add("spin");
  invoke("refresh");
});

$("#btn-settings").addEventListener("click", (e) => {
  const open = settingsView.hidden;
  e.currentTarget.setAttribute("aria-expanded", String(open));
  settingsView.hidden = !open;
  list.hidden = open;
  $(".title").textContent = open ? "Ajustes" : "Dispositivos";
  $("#summary").hidden = open;
  const shown = open ? settingsView : list;
  shown.classList.remove("swap-in");
  void shown.offsetWidth;
  shown.classList.add("swap-in");
  if (open) renderSettings();
});

settingsView.addEventListener("change", (e) => {
  const k = e.target.dataset.key;
  if (!k) return;
  const v = k === "mode" ? (e.target.checked ? "floating" : "flyout") : e.target.checked;
  invoke("update_settings", { patch: { [k]: v } });
});

$("#btn-quit").addEventListener("click", () => invoke("quit"));

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") dismiss();
  if (e.key === "F5" || (e.ctrlKey && e.key.toLowerCase() === "r")) { e.preventDefault(); invoke("refresh"); }
});
document.addEventListener("contextmenu", (e) => e.preventDefault());

listen("devices", (e) => { devices = e.payload; render(); });
listen("settings", (e) => { settings = e.payload; renderSettings(); });
listen("shown", enter);
listen("dismiss", dismiss);

// Relative times ("há 12 min") drift without new readings; refresh them quietly.
setInterval(render, 30_000);

// Also re-sync on every open: the hidden flyout's renderer is suspended and may miss events.
function sync() {
  return invoke("snapshot").then((s) => {
    skew = s.now - Date.now() / 1000;
    devices = s.devices;
    settings = s.settings;
    render();
    renderSettings();
  });
}
listen("shown", sync);
// Let the first frame paint with data before the window is revealed.
sync().finally(() => requestAnimationFrame(() => requestAnimationFrame(() => invoke("ready"))));
