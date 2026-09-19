// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
// GlassChain demo entry point: state, snapshot stream, commands, panels,
// resizable surfaces and the inspection drawer. Vanilla ES modules, no build
// step, no dependencies.
//
// The backend owns the experiment; this page only presents it. Every value
// shown comes from the headless runner's snapshot, never from animation
// timestamps, and a dropped stream reconnects by refetching the authoritative
// snapshot — the presentation stream is not an audit log.

import {
  badge,
  el,
  empty,
  fmtInt,
  fmtMoney,
  kpi,
  pdcTable,
  renderCompliance,
  renderInventory,
  renderOverview,
  renderPerformance,
  renderSecurity,
  renderTraceability,
  renderTrade,
  roleLabel,
  timeline,
} from "./views.js";
import {
  drawStats,
  initGraph,
  setGraphVisible,
  setHighlight,
  updateGraph,
} from "./graph.js";

const $ = (id) => document.getElementById(id);
const PANELS = ["overview", "inventory", "trade", "traceability", "security", "compliance", "performance"];
const RENDERERS = {
  overview: renderOverview,
  inventory: renderInventory,
  trade: renderTrade,
  traceability: renderTraceability,
  security: renderSecurity,
  compliance: renderCompliance,
  performance: renderPerformance,
};

const state = {
  token: "",
  mode: "",
  panel: "overview",
  view: "",
  snapshot: null,
  orgView: null,
  orgViewFor: "",
  orgs: [],
  params: null,
};

let selectSignature = "";
let rendererLabel = "";
let source = null;

// ── API helpers (token + Origin gated server-side; no cookies) ─────────────

async function getJSON(path) {
  const response = await fetch(path);
  if (!response.ok) throw new Error(`${response.status} ${path}`);
  return response.json();
}

async function postJSON(path, body) {
  const response = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json", "x-glass-auth": state.token },
    body: JSON.stringify(body),
  });
  let data = {};
  try {
    data = await response.json();
  } catch {
    data = {};
  }
  return { ok: response.ok, status: response.status, data };
}

function toast(message, tone) {
  const node = el("div", { class: "toast", dataset: tone ? { tone } : {}, text: message });
  $("toasts").append(node);
  setTimeout(() => node.remove(), 4200);
}

// ── Resizable surfaces ─────────────────────────────────────────────────────
// Every table scroll container and the drawer can be resized by pointer drag
// or keyboard (the handles are separators with arrow-key support); sizes
// persist in localStorage so the operator's layout survives reloads.

function readSizes() {
  try {
    return JSON.parse(localStorage.getItem("gc-resize") || "{}");
  } catch {
    return {};
  }
}

const sizes = readSizes();

function persistSize(key, value) {
  sizes[key] = Math.round(value);
  try {
    localStorage.setItem("gc-resize", JSON.stringify(sizes));
  } catch {
    // Storage unavailable (private mode) — resizing still works this session.
  }
}

function makeVerticalResizable(wrap, key) {
  if (wrap.dataset.resizable === "done") return;
  wrap.dataset.resizable = "done";
  const handle = el("div", {
    class: "resize-handle",
    role: "separator",
    tabindex: "0",
    "aria-orientation": "horizontal",
    "aria-label": "Resize table height",
    title: "Drag or use arrow keys to resize",
  });
  const apply = (height) => {
    const clamped = Math.max(48, Math.min(window.innerHeight * 0.9, height));
    wrap.style.maxHeight = `${Math.round(clamped)}px`;
    persistSize(key, clamped);
  };
  if (sizes[key]) wrap.style.maxHeight = `${sizes[key]}px`;
  handle.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    const startY = event.clientY;
    const startHeight = wrap.getBoundingClientRect().height;
    const move = (moveEvent) => apply(startHeight + (moveEvent.clientY - startY));
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  });
  handle.addEventListener("keydown", (event) => {
    const step = event.key === "PageUp" || event.key === "PageDown" ? 64 : 16;
    if (event.key === "ArrowUp" || event.key === "PageUp") apply(wrap.getBoundingClientRect().height - step);
    else if (event.key === "ArrowDown" || event.key === "PageDown") apply(wrap.getBoundingClientRect().height + step);
    else return;
    event.preventDefault();
  });
  wrap.after(handle);
}

function attachTableResizers(root = document) {
  for (const wrap of root.querySelectorAll(".table-wrap")) {
    const key = `table:${wrap.querySelector("table")?.id || wrap.dataset.key || "anon"}`;
    if (root === document && wrap.closest("#drawer")) continue;
    makeVerticalResizable(wrap, wrap.dataset.key || key);
  }
}

function attachDrawerResize() {
  const drawer = $("drawer");
  const handle = $("drawer-resize");
  const saved = Number(localStorage.getItem("gc-drawer-w") || 0);
  if (saved) drawer.style.width = `${saved}px`;
  const apply = (width) => {
    const clamped = Math.max(320, Math.min(window.innerWidth * 0.92, width));
    drawer.style.width = `${Math.round(clamped)}px`;
    try {
      localStorage.setItem("gc-drawer-w", String(Math.round(clamped)));
    } catch {
      // Storage unavailable — resizing still works this session.
    }
  };
  handle.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    const startWidth = drawer.getBoundingClientRect().width;
    const move = (moveEvent) => apply(startWidth + (startX - moveEvent.clientX));
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  });
  handle.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    apply(drawer.getBoundingClientRect().width + (event.key === "ArrowLeft" ? 32 : -32));
    event.preventDefault();
  });
}

// ── Bootstrap ──────────────────────────────────────────────────────────────

async function bootstrap() {
  const data = await getJSON("/api/bootstrap");
  state.token = data.token;
  state.mode = data.mode;
  state.params = data.params;
  $("mode").textContent = data.mode;
  $("mode").title = data.mode;
  fillParams(data.params);
  state.orgs = data.orgs.map(([id, role]) => ({ id, role, evil: false, member_of: [] }));
  syncViewSelect(state.orgs);
}

function fillParams(params) {
  const form = $("params");
  for (const [key, value] of Object.entries(params || {})) {
    const input = form.elements[key];
    if (input) input.value = value;
  }
}

function syncViewSelect(orgs) {
  const select = $("view-select");
  const signature = JSON.stringify([orgs.map((org) => [org.id, org.role, org.evil]), state.view]);
  if (signature === selectSignature) return;
  selectSignature = signature;
  const current = state.view;
  select.replaceChildren(el("option", { value: "", text: "Public — the chain only" }));
  for (const org of orgs) {
    select.append(
      el("option", {
        value: org.id,
        text: `${org.id} (${org.role})${org.evil ? " — evil" : ""}`,
      }),
    );
  }
  select.append(el("option", { value: "admin", text: "Admin (demo) — sees everything" }));
  const next = current && orgs.some((org) => org.id === current) ? current : current === "admin" ? "admin" : "";
  select.value = next;
  if (state.view !== next) {
    state.view = next;
    state.orgView = null;
    state.orgViewFor = "";
    orgFetchedAt = 0;
  }
}

// ── Snapshot stream ────────────────────────────────────────────────────────

function setConnection(value) {
  $("conn").dataset.state = value;
  $("conn-label").textContent = value;
}

function subscribe() {
  source = new EventSource("/api/events");
  source.addEventListener("snapshot", (message) => {
    setConnection("live");
    try {
      applySnapshot(JSON.parse(message.data));
    } catch (error) {
      console.error("snapshot render failed:", error);
    }
  });
  source.onerror = () => {
    setConnection("reconnecting");
    source.close();
    // A dropped or throttled stream is repaired with an authoritative
    // snapshot refetch; backend counters never rely on browser-facing state.
    refetch()
      .catch(() => {})
      .finally(() => setTimeout(subscribe, 1500));
  };
}

async function refetch() {
  const data = await getJSON("/api/snapshot");
  setConnection("live");
  applySnapshot(data);
}

function applySnapshot(snapshot) {
  state.snapshot = snapshot;
  if (snapshot.orgs && snapshot.orgs.length) {
    state.orgs = snapshot.orgs;
    syncViewSelect(snapshot.orgs);
  }
  const status = snapshot.status || "idle";
  $("run-status").dataset.state = status;
  $("run-label").textContent = status;
  updateGraph(snapshot);
  renderActivePanel();
  renderRendererInfo();
  applyMemberHighlight();
  // The admin lens carries the whole collection, so it is fetched on demand
  // by the drawers that need it; member views are small and refresh with the
  // snapshot cadence while the matrix that displays them is on screen.
  if (state.view && state.view !== "admin" && state.panel === "security") refreshOrg();
}

// One in-flight viewer-scoped fetch at a time, tagged with the view it
// belongs to. Responses are never discarded because another request started —
// only a view change invalidates them (a re-fetch loop that dropped every
// response under load was exactly how the admin lens used to end up empty).
const VIEW_FRESH_MS = 2000;
let orgFetch = null;
let orgFetchView = "";
let orgFetchedAt = 0;

function hasFreshOrgView() {
  return state.orgView && state.orgViewFor === state.view && Date.now() - orgFetchedAt < VIEW_FRESH_MS;
}

async function refreshOrg(force = false) {
  const view = state.view;
  if (!view) {
    state.orgView = null;
    state.orgViewFor = "";
    orgFetchedAt = 0;
    return null;
  }
  if (orgFetch && orgFetchView === view) return orgFetch;
  if (!force && hasFreshOrgView()) return state.orgView;
  orgFetchView = view;
  // ponytail: whole-lens fetch, throttled to one in-flight request; filter by
  // lot server-side if the collection ever grows enough for this to hurt.
  orgFetch = (async () => {
    try {
      const data = await getJSON(
        `/api/snapshot?as=${encodeURIComponent(view)}&viewer=${encodeURIComponent(view)}`,
      );
      if (state.view !== view) return null;
      state.orgView = data;
      state.orgViewFor = view;
      orgFetchedAt = Date.now();
      renderActivePanel();
      applyMemberHighlight();
      return data;
    } catch (error) {
      console.warn("view refresh failed:", error);
      return null;
    } finally {
      if (orgFetchView === view) {
        orgFetch = null;
        orgFetchView = "";
      }
    }
  })();
  return orgFetch;
}

// ── Panels ─────────────────────────────────────────────────────────────────

function setPanel(name) {
  state.panel = name;
  for (const panel of PANELS) $(`panel-${panel}`).hidden = panel !== name;
  for (const button of $("nav").children) {
    button.setAttribute("aria-current", button.dataset.panel === name ? "page" : "false");
  }
  renderActivePanel();
}

function renderActivePanel() {
  if (!state.snapshot) return;
  RENDERERS[state.panel]({
    snapshot: state.snapshot,
    orgView: state.orgView,
    view: state.view,
    selectedMember: state.view,
    openMember: openMemberDrawer,
    openLot: openLotDrawer,
    openCert: openCertDrawer,
    openBlock: openBlockDrawer,
    openContract: openContractDrawer,
    openPurchase: openPurchaseDrawer,
    openSecurity: openSecurityDrawer,
    buy: buyOffer,
  });
  applyMemberHighlight();
}

function applyMemberHighlight() {
  for (const row of document.querySelectorAll("[data-member]")) {
    row.classList.toggle("selected", row.dataset.member === state.view);
  }
}

function renderRendererInfo() {
  const { p99, samples, budget, backend, note } = drawStats();
  if (note) {
    if (note === rendererLabel) return;
    rendererLabel = note;
    $("renderer").textContent = note;
    return;
  }
  if (samples < 30) {
    const initial = `Canvas2D baseline · ${budget} ms p99 budget at the target load`;
    if (initial !== rendererLabel) {
      rendererLabel = initial;
      $("renderer").textContent = initial;
    }
    return;
  }
  const verdict = p99 <= budget ? "within" : "over";
  const label = `${backend} · draw p99 ${p99.toFixed(1)} ms · ${verdict} the ${budget} ms budget`;
  if (label === rendererLabel) return;
  rendererLabel = label;
  $("renderer").textContent = label;
}

// ── Viewing as (viewer-scoped views) ───────────────────────────────────────

const VIEW_TEXT = {
  public:
    "the default view — the public chain only: no private payload cleartext, commitments only.",
  member:
    "this page shows what that member can see: its own terms in cleartext, other members' terms as commitments.",
  admin:
    "demo lens — every payload's cleartext and every company's cash balance. The chain itself carries no global read.",
};

function updateBanner() {
  const view = state.view;
  const kind = view === "admin" ? "admin" : view ? "member" : "public";
  $("banner").dataset.kind = kind;
  $("banner-name").textContent = view === "admin" ? "Admin (demo)" : view || "Public";
  $("banner-text").textContent = VIEW_TEXT[kind];
  $("view-hint").textContent = VIEW_TEXT[kind];
}

async function selectView(value) {
  state.view = value;
  state.orgView = null;
  state.orgViewFor = "";
  orgFetchedAt = 0;
  $("view-select").value = value;
  updateBanner();
  applyMemberHighlight();
  if (!value) {
    closeDrawer();
    renderActivePanel();
    return;
  }
  await refreshOrg(true);
  renderActivePanel();
  openMemberDrawer(value);
}

// ── Drawer ─────────────────────────────────────────────────────────────────

function openDrawerShell(title, badges, bodyNodes) {
  $("drawer-title").textContent = title;
  $("drawer-badges").replaceChildren(...badges.filter(Boolean));
  $("drawer-body").replaceChildren(...bodyNodes);
  attachTableResizers($("drawer-body"));
  const drawer = $("drawer");
  const scrim = $("scrim");
  drawer.hidden = false;
  scrim.hidden = false;
  requestAnimationFrame(() => {
    drawer.classList.add("open");
    scrim.classList.add("open");
  });
}

function closeDrawer() {
  const drawer = $("drawer");
  const scrim = $("scrim");
  drawer.classList.remove("open");
  scrim.classList.remove("open");
  setHighlight(null);
  setTimeout(() => {
    if (!drawer.classList.contains("open")) {
      drawer.hidden = true;
      scrim.hidden = true;
    }
  }, 240);
}

function note(text) {
  return el("p", { class: "drawer-note", text });
}

function pairs(rows) {
  return el(
    "div",
    { class: "run-info" },
    ...rows.map(([label, value]) =>
      el("div", { class: "pair" }, el("dt", { text: label }), el("dd", { text: String(value) })),
    ),
  );
}

function donutRow(parts) {
  const total = parts.reduce((sum, part) => sum + part.value, 0);
  const donut = el("div", { class: "donut", role: "img", "aria-label": "Order status share" });
  let cursor = 0;
  const stops = [];
  for (const part of parts) {
    const from = (cursor / Math.max(1, total)) * 100;
    cursor += part.value;
    const to = (cursor / Math.max(1, total)) * 100;
    stops.push(`${part.color} ${from}% ${to}%`);
  }
  donut.style.background = total > 0 ? `conic-gradient(${stops.join(", ")})` : "var(--surface-3)";
  const legend = el(
    "ul",
    { class: "legend-list" },
    ...parts.map((part) => {
      const dot = el("i");
      dot.style.background = part.color;
      return el(
        "li",
        {},
        dot,
        el("span", { text: part.label }),
        el("span", {
          class: "pct",
          text: total > 0 ? `${Math.round((part.value / total) * 100)}%` : "0%",
        }),
      );
    }),
  );
  return el("div", { class: "donut-row" }, donut, legend);
}

function drawerSection(title, ...children) {
  return el("section", { class: "drawer-section" }, el("h3", { text: title }), ...children);
}

const isPendingOffer = (offer) =>
  offer.note.includes("awaiting") || offer.note.includes("still on offer");

function orderRows(data) {
  const sells = (data.sell_offers || []).map((offer) => {
    const pending = isPendingOffer(offer);
    return {
      kind: "Sell",
      ref: offer.tx_id,
      product: offer.product,
      counterparty: offer.buyer || "—",
      units: `${fmtInt(offer.quantity - (offer.sold || 0))} of ${fmtInt(offer.quantity)}`,
      price: fmtMoney(offer.price_per_unit),
      round: offer.round,
      status: pending ? ["Pending", "warn"] : ["Concluded", "good"],
    };
  });
  const buys = (data.purchases || []).map((order) => ({
    kind: "Buy",
    ref: order.tx_id,
    product: order.product,
    counterparty: order.seller,
    units: fmtInt(order.quantity),
    price: fmtMoney(order.price_per_unit),
    round: order.round,
    status: ["Concluded", "good"],
  }));
  return [...sells, ...buys].sort((left, right) => (right.round || 0) - (left.round || 0));
}

async function openMemberDrawer(org) {
  const meta = state.orgs.find((candidate) => candidate.id === org);
  const badges = [
    badge(roleLabel(meta?.role), "muted"),
    badge(meta?.evil ? "evil — not a member" : "member", meta?.evil ? "danger" : "good"),
  ];
  openDrawerShell(org, badges, [empty("Loading that member's own node…")]);

  const viewer = state.view;
  const viewerParam = viewer ? `&viewer=${encodeURIComponent(viewer)}` : "";
  const data = await getJSON(
    `/api/snapshot?as=${encodeURIComponent(org)}${viewerParam}`,
  ).catch(() => null);
  if (!data) {
    openDrawerShell(org, badges, [note("That member's node could not be reached.")]);
    return;
  }
  if (data.error) {
    openDrawerShell(org, badges, [note(data.error)]);
    return;
  }

  const pdcValues = data.pdc_values || [];
  const cash = data.cash;
  const sections = [];
  sections.push(
    note(
      `Read from ${org}'s own node — its ledger height is ${fmtInt(data.chain_height)}, the same as every synced member. Nothing here is the leader's view filtered.`,
    ),
  );

  const sells = data.sell_offers || [];
  const buys = data.purchases || [];
  const pendingSells = sells.filter(isPendingOffer);
  sections.push(
    drawerSection(
      "Orders",
      data.private_visible
        ? el(
            "div",
            { class: "kpis" },
            kpi("Concluded sells", fmtInt(sells.length - pendingSells.length), "filled or partially filled"),
            kpi("Pending sells", fmtInt(pendingSells.length), "awaiting a buyer"),
            kpi("Concluded buys", fmtInt(buys.length), "committed PurchaseOrders"),
            kpi("Inbound", fmtInt(data.pending_inbound ?? 0), "lots in transit to this member"),
          )
        : note(
            "This member's cash and trade bookkeeping are private — visible to the member itself, the regulator and Admin (demo). Switch to one of those lenses to inspect them.",
          ),
    ),
    data.private_visible
      ? donutRow([
          { label: "Concluded sells", value: sells.length - pendingSells.length, color: "#6e79e4" },
          { label: "Pending sells", value: pendingSells.length, color: "#e5a54b" },
          { label: "Buys", value: buys.length, color: "#4cb782" },
        ])
      : null,
  );

  const orders = orderRows(data);
  if (orders.length) {
    sections.push(
      tableSection(
        "Recent orders",
        ["Type", "Ref", "Product", "Counterparty", "Units", "Unit price", "Round", "Status"],
        orders.slice(0, 12).map((order) =>
          el(
            "tr",
            {},
            el("td", { text: order.kind }),
            el("td", { class: "mono", text: order.ref ? `${order.ref.slice(0, 8)}…` : "—" }),
            el("td", { text: order.product }),
            el("td", { text: order.counterparty || "—" }),
            el("td", { class: "num", text: order.units }),
            el("td", { class: "num", text: order.price }),
            el("td", { class: "num", text: fmtInt(order.round) }),
            el("td", {}, badge(order.status[0], order.status[1])),
          ),
        ),
        `orders:${org}`,
      ),
    );
  }

  const heldLots = (state.snapshot?.lots || []).filter(
    (lot) => lot.chain.length && lot.chain[lot.chain.length - 1].custodian === org,
  );
  sections.push(
    drawerSection(
      "Warehouse and cash",
      pairs([
        [
          "Trust score",
          meta && meta.records > 0
            ? `${meta.trust_score}/100 over ${fmtInt(meta.records)} registrations`
            : "no registrations yet — undefined, not assumed good",
        ],
        ["Stock on hand", `${fmtInt(data.stock?.lots_here ?? 0)} lots · ${fmtInt(data.stock?.units_here ?? 0)} units`],
        ["Receipts", `${fmtInt(data.stock?.received_units ?? 0)} units received`],
        ["Dispatches", `${fmtInt(data.stock?.dispatched_units ?? 0)} units dispatched`],
        ["Retail sold", `${fmtInt(data.stock?.sold_units ?? 0)} units`],
        ["Sellable pool", `${fmtInt(data.inventory ?? 0)} units`],
        [
          "Cash (demo)",
          cash == null
            ? "private in this view"
            : `${fmtMoney(cash)} — demo bookkeeping, the chain carries no balances`,
        ],
      ]),
    ),
  );
  if (heldLots.length) {
    sections.push(
      tableSection(
        "Inventory by lot",
        ["SKU", "Lot", "Units", "Acquired at", "Source"],
        heldLots.slice(-10).reverse().map((lot) => {
          const last = lot.chain[lot.chain.length - 1];
          return el(
            "tr",
            {},
            el("td", { text: "SKU-DEMO" }),
            el("td", { text: lot.lot_ref }),
            el("td", { class: "num", text: "500" }),
            el("td", { class: "num", text: `block ${last.block}` }),
            el("td", { text: lot.manufacturer || "—" }),
          );
        }),
        `inventory:${org}`,
      ),
    );
  }

  // The PDC table is capped to the latest rows: a long run authors hundreds
  // of payloads per member and rendering them all was the slow inspect path.
  const shownValues = pdcValues.slice(-40).reverse();
  const truncated = pdcValues.length > shownValues.length
    ? ` Showing the latest ${fmtInt(shownValues.length)}.`
    : "";
  sections.push(
    drawerSection(
      "Private payloads",
      note(
        pdcValues.length
          ? `Payloads authored by ${org} in the pricing collection: ${fmtInt(pdcValues.length)}.${truncated} Readable rows are held in ${org}'s own node; commitments are all the chain itself carries.`
          : `No private payload authored by ${org} has been disseminated yet.`,
      ),
      pdcValues.length ? pdcTable(shownValues, viewer) : empty("Nothing to inspect."),
    ),
  );

  openDrawerShell(org, badges, sections.filter(Boolean));
}

function tableSection(title, headers, rows, key) {
  const wrap = el(
    "div",
    { class: "table-wrap", dataset: { key } },
    el(
      "table",
      { class: "data" },
      el("thead", {}, el("tr", {}, ...headers.map((head) => el("th", { text: head })))),
      el("tbody", {}, ...rows),
    ),
  );
  return drawerSection(title, wrap);
}

function payloadsForLot(lotRef) {
  const seq = Number(String(lotRef).replace(/\D/g, ""));
  const values =
    state.orgView && state.orgViewFor === state.view ? state.orgView.pdc_values || [] : [];
  return values.filter((value) => value.lot === seq);
}

async function openLotDrawer(lot) {
  if (state.view) await refreshOrg(true);
  const stages = lot.chain || [];
  const certs = (state.snapshot?.certs || []).filter(
    (cert) => cert.lot_ref.toLowerCase() === lot.lot_ref.toLowerCase(),
  );
  const badges = [
    badge(lot.status, lot.status === "complete" ? "accent" : "muted"),
    badge(`made by ${lot.manufacturer}`, "muted"),
    badge(lot.lineage_complete ? "lineage verified" : "lineage pending", lot.lineage_complete ? "good" : "muted"),
  ];
  const sections = [
    note(
      stages.length
        ? "A lot's journey takes about four rounds — each hop is committed by the handover company."
        : "No committed custody events yet.",
    ),
    drawerSection(
      "Compliance",
      pairs([
        ["SNCM schema", lot.schema_compliant ? "compliant · 0.7× gas" : "non-compliant · 1.0× gas"],
        ["Verifiable lineage", lot.lineage_complete ? "verified — mandatory custody events in order" : "pending custody events"],
        ["Custody events", fmtInt(stages.length)],
        ["Analytical records", fmtInt(lot.flat_records ?? 0)],
        ["Trust", `${lot.trust_score}/100 at origin · ${lot.trust_avg ?? lot.trust_score}/100 average`],
      ]),
    ),
  ];
  if (stages.length) {
    sections.push(drawerSection("Custody timeline", timeline(stages)));
    setHighlight(stages.map((stage) => stage.custodian));
  }
  sections.push(
    drawerSection(
      "Certifications and audits",
      certs.length
        ? el(
            "ul",
            { class: "feed" },
            ...certs.map((cert) =>
              el("li", {}, el("span", { text: `${cert.record_id} — ${cert.schema}` }), el("span", { class: "when", text: cert.issuer })),
            ),
          )
        : note("Certifications are issued the round after a lot is manufactured."),
    ),
  );
  const viewer = state.view;
  const payloads = payloadsForLot(lot.lot_ref);
  sections.push(
    drawerSection(
      "Private payloads — collection `pricing`",
      note(
        !viewer
          ? "Public lens: the chain holds only sha256 commitments — no cleartext is readable here."
          : `Viewing as ${viewer === "admin" ? "Admin (demo)" : viewer}: readable rows are the terms this viewer's node holds; the rest stay commitments.`,
      ),
      payloads.length
        ? pdcTable(payloads, viewer)
        : note("No payload for this lot is readable in this view yet."),
    ),
  );
  openDrawerShell(lot.lot_ref, badges, sections);
}

async function openCertDrawer(cert) {
  if (state.view) await refreshOrg(true);
  const lot = (state.snapshot?.lots || []).find(
    (candidate) => candidate.lot_ref.toLowerCase() === cert.lot_ref.toLowerCase(),
  );
  const badges = [badge(cert.schema.replaceAll("_", " "), "accent"), badge(cert.status, "good")];
  const sections = [pairs([["Record", cert.record_id], ["Lot", cert.lot_ref], ["Issuer", cert.issuer]])];
  if (lot && lot.chain.length) {
    sections.push(drawerSection("Related custody chain", timeline(lot.chain)));
    setHighlight(lot.chain.map((stage) => stage.custodian));
  } else {
    sections.push(note("The certified lot's custody chain has not committed yet."));
  }
  const payloads = payloadsForLot(cert.lot_ref);
  sections.push(
    drawerSection(
      "Private payloads",
      note(
        state.view
          ? `Readable to the current viewer: ${payloads.length} payload(s) for this lot.`
          : "Public lens: commitments only.",
      ),
      payloads.length ? pdcTable(payloads, state.view) : empty("Nothing readable in this view."),
    ),
  );
  openDrawerShell(cert.record_id, badges, sections);
}

function openBlockDrawer(block) {
  openDrawerShell(
    `Block ${block.height}`,
    [badge(block.certified ? "quorum certificate" : "PoW seal", block.certified ? "good" : "muted")],
    [
      pairs([
        ["Height", fmtInt(block.height)],
        ["Hash", block.hash],
        ["Previous hash", block.previous_hash],
        ["Transactions", fmtInt(block.tx_count)],
        ["Committed", new Date(block.timestamp * 1000).toLocaleString()],
      ]),
      note(
        "Each block stores the previous block's hash, so altering any committed transaction would break every link after it. The verifier re-derives and checks these links on sync and restart.",
      ),
    ],
  );
}

function openContractDrawer(contract) {
  const purchases = (state.snapshot?.offers || []).filter(
    (event) => event.kind === "purchase" && event.note.includes(contract.id),
  );
  const badges = [
    badge(contract.auto_execute ? "auto-execute" : "human approval", contract.auto_execute ? "good" : "accent"),
    badge(contract.status, "muted"),
  ];
  const sections = [
    pairs([
      ["Contract", contract.id],
      ["Buyer", contract.buyer],
      ["Product", contract.product],
      ["Price cap", `${fmtMoney(contract.max_price_per_unit)} per unit`],
      ["Lifetime cap", `${fmtInt(contract.quantity_purchased)} of ${fmtInt(contract.max_quantity)} units`],
      ["Executions", fmtInt(contract.executions)],
    ]),
    note(
      contract.auto_execute
        ? "Auto-executing: every matching SupplyOffer under this price cap produces a real PurchaseOrder. Bands never overlap between auto contracts, so one offer can only execute once."
        : "Conditions-only: matching offers are advertised, never bought automatically. A human completes one against this contract through the buyer's own node.",
    ),
  ];
  if (purchases.length) {
    sections.push(
      tableSection(
        "Committed purchases",
        ["Tx", "Seller", "Buyer", "Qty", "Unit price", "Round"],
        purchases.slice(-12).reverse().map((order) =>
          el(
            "tr",
            {},
            el("td", { class: "mono", text: `${order.tx_id.slice(0, 10)}…` }),
            el("td", { text: order.seller }),
            el("td", { text: order.buyer }),
            el("td", { class: "num", text: fmtInt(order.quantity) }),
            el("td", { class: "num", text: fmtMoney(order.price_per_unit) }),
            el("td", { class: "num", text: fmtInt(order.round) }),
          ),
        ),
        `contract:${contract.id}`,
      ),
    );
  } else {
    sections.push(note("No committed purchases against this contract yet."));
  }
  openDrawerShell(contract.id, badges, sections);
}

function openPurchaseDrawer(order) {
  const contract = (state.snapshot?.contracts || []).find((candidate) =>
    order.note.includes(candidate.id),
  );
  const manual = order.note.includes("manual");
  const total = order.quantity * order.price_per_unit;
  openDrawerShell(
    `Purchase ${order.tx_id.slice(0, 10)}…`,
    [badge(manual ? "manual" : "auto", manual ? "accent" : "good"), badge(`round ${fmtInt(order.round)}`, "muted")],
    [
      pairs([
        ["Transaction", order.tx_id],
        ["Product", order.product],
        ["Seller", order.seller],
        ["Buyer", order.buyer],
        ["Quantity", `${fmtInt(order.quantity)} units`],
        ["Unit price", fmtMoney(order.price_per_unit)],
        ["Total", fmtMoney(total)],
        ["Contract", contract ? contract.id : "—"],
      ]),
      note(order.note),
      note(
        "A real PurchaseOrder: submitted on the buyer's node, relayed, admitted and committed on-chain — the same path as every other transaction.",
      ),
    ],
  );
}

function openTxDrawer(tx) {
  openDrawerShell(
    `Transaction ${tx.id.slice(0, 10)}…`,
    [badge(tx.kind, "accent"), badge(`h${tx.height}`, "muted")],
    [
      pairs([
        ["Id", tx.id],
        ["Kind", tx.kind],
        ["Flow", `${tx.from} → ${tx.to}`],
        ["Committed height", fmtInt(tx.height)],
      ]),
      note(tx.label),
      note(
        "Every dot on the graph is a real transaction: submitted on its origin member's node, relayed, admitted, then committed on-chain.",
      ),
    ],
  );
}

function openSecurityDrawer(event) {
  const outcome = String(event.outcome || "");
  openDrawerShell(
    event.actor,
    [badge(outcome, outcome.includes("admitted") ? "warn" : "good")],
    [
      pairs([
        ["Attack", event.action],
        ["Outcome", outcome],
        ["Enforced by", event.detail],
      ]),
      note(String(event.explanation || "").replace(/\s+/g, " ").trim()),
    ],
  );
}

async function buyOffer(offer, quantity) {
  // The offer names the buyer whose conditions-only contract matched it;
  // buying through that pharmacy's own node keeps the flow honest.
  const buyer = offer.buyer || (/^pharmacy-/.test(state.view) ? state.view : "pharmacy-1");
  const response = await postJSON("/api/purchase", {
    offer_tx_id: offer.tx_id,
    buyer,
    quantity,
  });
  if (response.ok) {
    toast(`PurchaseOrder submitted as ${buyer} · ${quantity} units`, "good");
  } else {
    toast(response.data.error || "Purchase rejected", "error");
  }
}

// ── Wiring ─────────────────────────────────────────────────────────────────

$("nav").addEventListener("click", (event) => {
  const button = event.target.closest("button[data-panel]");
  if (button) setPanel(button.dataset.panel);
});

$("btn-start").addEventListener("click", async () => {
  const response = await postJSON("/api/run", { action: "start" });
  if (!response.ok) toast(response.data.error || "Start rejected", "error");
});
$("btn-stop").addEventListener("click", async () => {
  const response = await postJSON("/api/run", { action: "stop" });
  if (!response.ok) toast(response.data.error || "Stop rejected", "error");
});
$("btn-reset").addEventListener("click", () => $("reset-dialog").showModal());
$("reset-dialog").addEventListener("close", async (event) => {
  if (event.target.returnValue !== "reset") return;
  const response = await postJSON("/api/run", { action: "reset" });
  if (response.ok) {
    closeDrawer();
    toast("Run reset", "good");
  } else {
    toast(response.data.error || "Reset rejected", "error");
  }
});

async function applyParams(form) {
  const payload = {};
  for (const input of form.elements) {
    if (input.name) payload[input.name] = Number(input.value);
  }
  const response = await postJSON("/api/params", payload);
  const noteNode = $("params-note");
  if (response.ok) {
    fillParams(response.data.params);
    noteNode.textContent = "applied — topology changes rebuild at the next round boundary";
    toast("Simulation parameters applied", "good");
  } else {
    noteNode.textContent = `rejected: ${response.data.error || "unknown error"}`;
    toast(response.data.error || "Parameters rejected", "error");
  }
  setTimeout(() => {
    noteNode.textContent = "";
  }, 6000);
}

$("params").addEventListener("submit", (event) => {
  event.preventDefault();
  applyParams(event.target);
});

$("btn-stress").addEventListener("click", () => {
  const form = $("params");
  form.elements.lots_per_round.value = "50";
  form.elements.round_interval_ms.value = "0";
  form.elements.evil_nodes.value = "2";
  applyParams(form);
});

$("view-select").addEventListener("change", (event) => selectView(event.target.value));
$("btn-public").addEventListener("click", () => selectView(""));

$("btn-theme").addEventListener("click", () => {
  const next = document.documentElement.dataset.theme === "light" ? "dark" : "light";
  document.documentElement.dataset.theme = next;
  localStorage.setItem("gc-theme", next);
  $("theme-icon").setAttribute("href", next === "dark" ? "#i-sun" : "#i-moon");
});

// The federation graph is shared by every panel; the rail keeps it on screen
// and the header toggle collapses it for readers who want the table space.
let graphCollapsed = localStorage.getItem("gc-graph-collapsed") === "1";
function applyGraphCollapsed() {
  $("graph-card").classList.toggle("collapsed", graphCollapsed);
  $("btn-graph").setAttribute("aria-expanded", String(!graphCollapsed));
  setGraphVisible(!graphCollapsed);
}
$("btn-graph").addEventListener("click", () => {
  graphCollapsed = !graphCollapsed;
  try {
    localStorage.setItem("gc-graph-collapsed", graphCollapsed ? "1" : "0");
  } catch {
    // Storage unavailable — the collapse still works this session.
  }
  applyGraphCollapsed();
});

$("drawer-close").addEventListener("click", closeDrawer);
$("scrim").addEventListener("click", closeDrawer);
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("drawer").hidden) closeDrawer();
});

// ── Boot ───────────────────────────────────────────────────────────────────

document.documentElement.dataset.theme = localStorage.getItem("gc-theme") || "dark";
$("theme-icon").setAttribute(
  "href",
  document.documentElement.dataset.theme === "dark" ? "#i-sun" : "#i-moon",
);

try {
  await bootstrap();
} catch (error) {
  toast(`Bootstrap failed — is the demo server running? (${error.message})`, "error");
}
initGraph(
  { canvas: $("graph"), overlay: $("gpu"), tooltip: $("graph-tip") },
  { onNode: openMemberDrawer, onTx: openTxDrawer },
);
applyGraphCollapsed();
attachTableResizers();
attachDrawerResize();
setPanel("overview");
updateBanner();
refetch().catch(() => setConnection("reconnecting"));
subscribe();
