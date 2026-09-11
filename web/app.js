"use strict";

/**
 * LANdrop dashboard — vanilla JS, no build step.
 *
 * Responsibilities:
 *  - PIN gate (if the server requires one)
 *  - Fetching status / QR / devices / history from the REST API
 *  - Drag & drop + click-to-select uploads with live progress (XHR, since
 *    fetch() doesn't expose upload progress events)
 *  - A WebSocket connection that keeps everything in sync in real time
 *  - Accept/reject flow for incoming files
 *  - Dark/light theme toggle
 */

const state = {
  token: sessionStorage.getItem("landrop_token") || null,
  pinRequired: false,
  activeUploads: new Map(), // transferId -> DOM row element
  ws: null,
};

// ---------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------

document.addEventListener("DOMContentLoaded", init);

async function init() {
  initTheme();
  wireDropzone();
  wirePinForm();

  try {
    const status = await apiFetch("/api/status", { skipAuth: true });
    state.pinRequired = status.pin_required;

    if (state.pinRequired && !state.token) {
      showPinGate();
      return;
    }

    await enterApp();
  } catch (err) {
    console.error("failed to load initial status", err);
    showToast("Could not reach LANdrop server.", "error");
  }
}

async function enterApp() {
  document.getElementById("pin-gate").classList.add("hidden");
  document.getElementById("app").classList.remove("hidden");

  await Promise.all([loadStatus(), loadQr(), loadDevices(), loadHistory()]);
  connectWebSocket();

  // Devices / status are refreshed periodically as a fallback in case a
  // WebSocket event is ever missed (e.g. reconnect race).
  setInterval(loadDevices, 8000);
}

// ---------------------------------------------------------------------
// PIN gate
// ---------------------------------------------------------------------

function showPinGate() {
  document.getElementById("pin-gate").classList.remove("hidden");
  document.getElementById("app").classList.add("hidden");
  document.getElementById("pin-input").focus();
}

function wirePinForm() {
  const form = document.getElementById("pin-form");
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const pin = document.getElementById("pin-input").value.trim();
    const errorEl = document.getElementById("pin-error");
    errorEl.classList.add("hidden");

    try {
      const res = await fetch("/api/auth", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ pin }),
      });
      if (!res.ok) {
        errorEl.classList.remove("hidden");
        return;
      }
      const data = await res.json();
      state.token = data.token;
      sessionStorage.setItem("landrop_token", data.token);
      await enterApp();
    } catch (err) {
      errorEl.textContent = "Could not reach server. Try again.";
      errorEl.classList.remove("hidden");
    }
  });
}

// ---------------------------------------------------------------------
// API helper
// ---------------------------------------------------------------------

async function apiFetch(path, opts = {}) {
  const headers = opts.headers || {};
  if (!opts.skipAuth && state.token) {
    headers["X-Landrop-Token"] = state.token;
  }
  const res = await fetch(path, { ...opts, headers });
  if (res.status === 401) {
    state.token = null;
    sessionStorage.removeItem("landrop_token");
    showPinGate();
    throw new Error("unauthorized");
  }
  if (!res.ok) {
    const body = await res.json().catch(() => ({ message: res.statusText }));
    throw new Error(body.message || "request failed");
  }
  const contentType = res.headers.get("content-type") || "";
  return contentType.includes("application/json") ? res.json() : res.text();
}

// ---------------------------------------------------------------------
// Status / QR / devices / history
// ---------------------------------------------------------------------

async function loadStatus() {
  const status = await apiFetch("/api/status", { skipAuth: true });
  const pill = document.getElementById("status-pill");
  const text = document.getElementById("status-pill__text");
  pill.classList.toggle("offline", !status.online);
  text.textContent = status.online ? "Online" : "Offline";
  document.getElementById("connect-url").textContent = status.address;
}

async function loadQr() {
  document.getElementById("qr-image").src = "/api/qr?_=" + Date.now();
}

async function loadDevices() {
  try {
    const devices = await apiFetch("/api/devices");
    renderDevices(devices);
  } catch (err) {
    // non-fatal — the panel just stays as-is
  }
}

async function loadHistory() {
  try {
    const history = await apiFetch("/api/history");
    renderHistory(history);
  } catch (err) {
    // non-fatal
  }
}

function renderDevices(devices) {
  const container = document.getElementById("devices-list");
  if (!devices.length) {
    container.innerHTML = '<p class="empty-state">No other devices discovered yet.</p>';
    return;
  }
  container.innerHTML = devices
    .map(
      (d) => `
      <div class="device-item">
        <span class="device-item__name">${iconForDevice(d.name)} ${escapeHtml(d.name)}</span>
        <span class="device-item__status ${d.connected ? "" : "offline"}">
          ${d.connected ? "Connected" : "Offline"}
        </span>
      </div>`
    )
    .join("");
}

function renderHistory(records) {
  const container = document.getElementById("history-list");
  if (!records.length) {
    container.innerHTML = '<p class="empty-state">No transfers yet. Drag a file in to get started.</p>';
    return;
  }
  container.innerHTML = records
    .map((r) => {
      const arrow = r.direction === "upload" ? "↑" : "↓";
      const arrowClass = r.direction === "upload" ? "up" : "down";
      return `
      <div class="history-item">
        <div class="history-item__left">
          <span class="direction-icon ${arrowClass}">${arrow}</span>
          <span class="history-item__name" title="${escapeHtml(r.filename)}">${escapeHtml(r.filename)}</span>
        </div>
        <div class="history-item__right">
          <span class="history-item__size">${formatBytes(r.size_bytes)}</span>
          <span class="badge badge--${r.status}">${capitalize(r.status)}</span>
        </div>
      </div>`;
    })
    .join("");
}

// ---------------------------------------------------------------------
// WebSocket — real-time updates
// ---------------------------------------------------------------------

function connectWebSocket() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  state.ws = ws;

  ws.onmessage = (event) => {
    try {
      const msg = JSON.parse(event.data);
      handleWsEvent(msg);
    } catch (err) {
      console.error("bad ws payload", err);
    }
  };

  ws.onclose = () => {
    setStatusOffline();
    setTimeout(connectWebSocket, 2000); // simple reconnect with backoff-free retry
  };

  ws.onerror = () => ws.close();
}

function setStatusOffline() {
  document.getElementById("status-pill").classList.add("offline");
  document.getElementById("status-pill__text").textContent = "Reconnecting…";
}

function handleWsEvent(msg) {
  const record = msg.payload;
  switch (msg.type) {
    case "transfer_started":
      upsertUploadRow(record);
      break;
    case "transfer_progress":
      upsertUploadRow(record);
      break;
    case "transfer_completed":
      upsertUploadRow(record);
      showToast(`${record.filename} completed`, "success");
      loadHistory();
      removeIncomingCard(record.id);
      break;
    case "transfer_failed":
      upsertUploadRow(record);
      showToast(`${record.filename} ${record.status === "rejected" ? "rejected" : "failed"}`, "error");
      loadHistory();
      removeIncomingCard(record.id);
      break;
    case "incoming_file":
      renderIncomingCard(record);
      showToast(`Incoming file: ${record.filename}`, "info");
      break;
    case "device_joined":
      showToast(`${msg.payload.name} connected`, "info");
      loadDevices();
      break;
    case "device_left":
      loadDevices();
      break;
    default:
      break;
  }
  loadStatus();
}

// ---------------------------------------------------------------------
// Upload (drag & drop + click)
// ---------------------------------------------------------------------

function wireDropzone() {
  const dropzone = document.getElementById("dropzone");
  const input = document.getElementById("file-input");

  dropzone.addEventListener("click", () => input.click());
  dropzone.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") input.click();
  });

  input.addEventListener("change", () => {
    uploadFiles(Array.from(input.files));
    input.value = "";
  });

  ["dragenter", "dragover"].forEach((evt) =>
    dropzone.addEventListener(evt, (e) => {
      e.preventDefault();
      dropzone.classList.add("dragover");
    })
  );

  ["dragleave", "drop"].forEach((evt) =>
    dropzone.addEventListener(evt, (e) => {
      e.preventDefault();
      dropzone.classList.remove("dragover");
    })
  );

  dropzone.addEventListener("drop", (e) => {
    const files = Array.from(e.dataTransfer.files || []);
    if (files.length) uploadFiles(files);
  });
}

function uploadFiles(files) {
  for (const file of files) {
    uploadOne(file);
  }
}

function uploadOne(file) {
  const clientId = "local-" + Math.random().toString(36).slice(2);
  const row = createUploadRow(clientId, file.name, file.size);
  document.getElementById("upload-list").prepend(row);

  const form = new FormData();
  form.append("direction", "upload"); // sharing our own file
  form.append("sender", "This device");
  form.append("file", file, file.name);

  const xhr = new XMLHttpRequest();
  xhr.open("POST", "/api/upload");
  if (state.token) xhr.setRequestHeader("X-Landrop-Token", state.token);

  const startedAt = Date.now();

  xhr.upload.addEventListener("progress", (e) => {
    if (!e.lengthComputable) return;
    const elapsedSec = Math.max((Date.now() - startedAt) / 1000, 0.001);
    const speed = e.loaded / elapsedSec;
    const remaining = speed > 0 ? (e.total - e.loaded) / speed : 0;
    updateUploadRow(row, {
      percent: (e.loaded / e.total) * 100,
      loaded: e.loaded,
      total: e.total,
      speed,
      remaining,
      state: "uploading",
    });
  });

  xhr.onload = () => {
    if (xhr.status >= 200 && xhr.status < 300) {
      updateUploadRow(row, { percent: 100, state: "completed" });
      loadHistory();
    } else {
      let message = "Upload failed";
      try {
        message = JSON.parse(xhr.responseText).message || message;
      } catch (_) {}
      updateUploadRow(row, { state: "failed", error: message });
      showToast(message, "error");
    }
  };

  xhr.onerror = () => {
    updateUploadRow(row, { state: "failed", error: "Network error" });
    showToast(`Failed to upload ${file.name}`, "error");
  };

  xhr.send(form);
}

function createUploadRow(id, name, size) {
  const el = document.createElement("div");
  el.className = "transfer-item";
  el.dataset.id = id;
  el.innerHTML = `
    <div class="transfer-item__row">
      <span class="transfer-item__name">${escapeHtml(name)}</span>
      <span class="badge badge--pending">Starting…</span>
    </div>
    <div class="transfer-item__meta">
      <span class="meta-size">${formatBytes(size)}</span>
      <span class="meta-speed"></span>
      <span class="meta-eta"></span>
    </div>
    <div class="progress-bar"><div class="progress-bar__fill" style="width:0%"></div></div>
  `;
  return el;
}

function updateUploadRow(row, { percent, loaded, total, speed, remaining, state: s, error }) {
  const fill = row.querySelector(".progress-bar__fill");
  const badge = row.querySelector(".badge");
  if (percent !== undefined) fill.style.width = `${Math.min(percent, 100)}%`;

  if (s === "uploading") {
    badge.textContent = `${Math.round(percent)}%`;
    badge.className = "badge badge--inprogress";
    row.querySelector(".meta-speed").textContent = `${formatBytes(speed)}/s`;
    row.querySelector(".meta-eta").textContent = `${Math.max(Math.round(remaining), 0)}s remaining`;
  } else if (s === "completed") {
    badge.textContent = "Completed";
    badge.className = "badge badge--completed";
    row.querySelector(".meta-speed").textContent = "";
    row.querySelector(".meta-eta").textContent = "Done";
  } else if (s === "failed") {
    badge.textContent = "Failed";
    badge.className = "badge badge--failed";
    row.querySelector(".meta-eta").textContent = error || "Error";
  }
}

/** Reflect a server-pushed TransferRecord for an already-known active row. */
function upsertUploadRow(record) {
  let row = document.querySelector(`.transfer-item[data-id="${record.id}"]`);
  if (!row) {
    row = createUploadRow(record.id, record.filename, record.size_bytes);
    document.getElementById("upload-list").prepend(row);
  }
  const percent = record.size_bytes > 0 ? (record.bytes_transferred / record.size_bytes) * 100 : 0;
  const statusMap = {
    pending: "pending",
    inprogress: "uploading",
    completed: "completed",
    failed: "failed",
    rejected: "failed",
    cancelled: "failed",
  };
  updateUploadRow(row, {
    percent,
    state: statusMap[record.status] || "uploading",
    error: record.error,
  });
}

// ---------------------------------------------------------------------
// Incoming files (accept / reject)
// ---------------------------------------------------------------------

function renderIncomingCard(record) {
  const section = document.getElementById("incoming-section");
  const list = document.getElementById("incoming-list");
  section.classList.remove("hidden");

  let card = document.querySelector(`.incoming-item[data-id="${record.id}"]`);
  if (!card) {
    card = document.createElement("div");
    card.className = "incoming-item";
    card.dataset.id = record.id;
    list.prepend(card);
  }

  card.innerHTML = `
    <div class="incoming-item__details">
      <div class="incoming-item__name">${escapeHtml(record.filename)}</div>
      <div class="incoming-item__meta">${formatBytes(record.size_bytes)} · from ${escapeHtml(record.sender)}</div>
    </div>
    <div class="incoming-item__actions">
      <button class="btn btn--primary btn--sm" data-action="accept">Accept</button>
      <button class="btn btn--danger btn--sm" data-action="reject">Reject</button>
    </div>
  `;

  card.querySelector('[data-action="accept"]').addEventListener("click", () => respond(record.id, "accept"));
  card.querySelector('[data-action="reject"]').addEventListener("click", () => respond(record.id, "reject"));
}

function removeIncomingCard(id) {
  const card = document.querySelector(`.incoming-item[data-id="${id}"]`);
  if (card) card.remove();
  const list = document.getElementById("incoming-list");
  if (!list.children.length) {
    document.getElementById("incoming-section").classList.add("hidden");
  }
}

async function respond(id, action) {
  try {
    await apiFetch(`/api/transfers/${id}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ action }),
    });
    removeIncomingCard(id);
  } catch (err) {
    showToast("Could not respond to incoming file", "error");
  }
}

// ---------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------

function initTheme() {
  const saved = localStorage.getItem("landrop_theme");
  const preferred = saved || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  document.documentElement.setAttribute("data-theme", preferred);

  document.getElementById("theme-toggle").addEventListener("click", () => {
    const current = document.documentElement.getAttribute("data-theme");
    const next = current === "dark" ? "light" : "dark";
    document.documentElement.setAttribute("data-theme", next);
    localStorage.setItem("landrop_theme", next);
  });
}

// ---------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------

function showToast(message, kind = "info") {
  const container = document.getElementById("toast-container");
  const toast = document.createElement("div");
  toast.className = `toast toast--${kind}`;
  toast.textContent = message;
  container.appendChild(toast);
  setTimeout(() => toast.remove(), 4000);
}

// ---------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------

function formatBytes(bytes) {
  if (!bytes || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function capitalize(s) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function iconForDevice(name) {
  const n = name.toLowerCase();
  if (n.includes("phone") || n.includes("android") || n.includes("iphone")) return "📱";
  if (n.includes("mac") || n.includes("laptop") || n.includes("book")) return "💻";
  return "🖥️";
}

function escapeHtml(str) {
  const div = document.createElement("div");
  div.textContent = str;
  return div.innerHTML;
}
