// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
// GlassChain demo frontend: vanilla ES modules, no build step.
//
// Renderer policy (gui-demo-benchmark.md §3, step-5 gate): Canvas2D is the
// baseline. WebGPU is attempted only when the measured Canvas2D frame time at
// the target load misses the 16.7 ms p95 budget, and it falls back
// automatically when no adapter is available or the device is lost.

const $ = (id) => document.getElementById(id);

const ROLE_COLORS = {
  manufacturer: [31, 163, 140],
  distributor: [58, 122, 200],
  logistics: [222, 138, 54],
  pharmacy: [149, 91, 199],
  regulator: [182, 173, 49],
  certifier: [110, 118, 122],
};
const EVIL_COLOR = [201, 60, 60];
const DOT_COLOR = [227, 232, 68];
const SEGMENT_COLORS = [
  "#6e79e4", "#4cb782", "#e0a458", "#c76fd1", "#5ab0d0",
  "#d3b354", "#9a8fe4", "#7ac0a0", "#c97979", "#a0b4c0",
];

const TARGET_LOAD = 500; // moving elements at the densest intended state
const FRAME_BUDGET_MS = 10; // the target load must animate under 10 ms p95

let token = "";
let latest = null;
let orgRoles = {};
let orgSnapshotData = null;
let canvas, ctx2d;
let measuring = false;
let dotLoad = 6;

// Appearance + motion preferences (persisted). Motion is reduced by
// default: the canvas animates slower and with fewer moving dots, and
// every panel only re-renders when its content actually changed.
const themePref = {
  value: localStorage.getItem("gc-theme") || "dark",
  apply() {
    document.documentElement.dataset.theme = this.value;
    $("btn-theme").textContent = this.value === "light" ? "☾" : "◐";
  },
  toggle() {
    this.value = this.value === "light" ? "dark" : "light";
    localStorage.setItem("gc-theme", this.value);
    this.apply();
  },
};
// Motion is permanently reduced: a slow, capped flow. No toggle — the
// screen must stay calm for reading and clicking.

// Drag offsets and hover state, keyed by company id.
const dragOffsets = {};
let hoverId = null;
let dragging = null;
let hoverTx = null;

// ── Session bootstrap ───────────────────────────────────────────────────────

async function bootstrap() {
  const response = await fetch("/api/bootstrap");
  const data = await response.json();
  token = data.token;
  $("mode-label").textContent = data.mode;
  orgRoles = Object.fromEntries(data.orgs.map((company) => [company[0], company[1]]));
  const form = $("params-form");
  for (const [key, value] of Object.entries(data.params)) {
    const input = form.elements[key];
    if (input) input.value = value;
  }
  const select = $("view-org");
  for (const [id, role] of data.orgs) {
    const option = document.createElement("option");
    option.value = id;
    option.textContent = `${id} (${role})`;
    select.append(option);
  }
  const admin = document.createElement("option");
  admin.value = "admin";
  admin.textContent = "Admin (demo) — sees everything";
  select.append(admin);
}

// ── Commands (token + Origin gated server-side; no cookies) ─────────────────

async function command(action) {
  const response = await fetch("/api/run", {
    method: "POST",
    headers: { "content-type": "application/json", "x-glass-auth": token },
    body: JSON.stringify({ action }),
  });
  if (!response.ok) {
    console.error("command rejected:", await response.text());
  }
}

$("params-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.target;
  const params = Object.fromEntries(
    [...form.elements].filter((el) => el.name).map((el) => [el.name, Number(el.value)]),
  );
  const response = await fetch("/api/params", {
    method: "POST",
    headers: { "content-type": "application/json", "x-glass-auth": token },
    body: JSON.stringify(params),
  });
  const result = await response.json();
  $("params-status").textContent = response.ok
    ? "applied — topology changes rebuild the federation automatically"
    : `rejected: ${result.error ?? "unknown error"}`;
});

$("btn-theme").addEventListener("click", () => themePref.toggle());
themePref.apply();
document.documentElement.dataset.motion = "flow";

// Self-heal a cached pre-restructure page: if the served HTML lacks the
// drawer-scroll-outer wrapper (stale page), rebuild it so the drawer body
// has its scroll strip back.
if (!$("drawer-scroll-outer")) {
  const drawer = $("member-drawer");
  const content = $("drawer-content");
  const outer = document.createElement("div");
  outer.id = "drawer-scroll-outer";
  content.replaceWith(outer);
  outer.append(content);
  drawer.insertBefore(outer, drawer.firstChild);
}
$("btn-start").addEventListener("click", () => command("start"));
$("btn-stop").addEventListener("click", () => command("stop"));
$("btn-reset").addEventListener("click", () => command("reset"));
$("view-org").addEventListener("change", () => {
  refetchSnapshot();
  const view = $("view-org").value;
  updateViewBanner(view);
  if (view) openMemberDrawer(view);
});
$("btn-clear-view").addEventListener("click", clearMemberView);
$("btn-banner-clear").addEventListener("click", clearMemberView);
updateViewBanner("");

function currentViewOrg() {
  return $("view-org").value;
}

function updateViewBanner(view) {
  const banner = $("view-banner");
  const name = $("view-banner-name");
  if (view === "admin") {
    $("view-banner-name").textContent = "Admin (demo)";
    $("view-banner-text").textContent =
      "Every payload's cleartext and every company's cash balance are visible. This lens exists for the demonstration only.";
    banner.classList.remove("hidden");
  } else if (view) {
    $("view-banner-name").textContent = view;
    $("view-banner-text").textContent =
      "This page shows what that member can see — your own terms in cleartext, other members' terms as commitments.";
    banner.classList.remove("hidden");
  } else {
    $("view-banner-name").textContent = "Public";
    $("view-banner-text").textContent =
      "the default view — the public chain only: no private payload cleartext, commitments only.";
    banner.classList.remove("hidden");
  }
  // Highlight the selected member wherever it is listed.
  for (const row of document.querySelectorAll("tr[data-member]")) {
    row.classList.toggle("selected", row.dataset.member === view);
  }
  for (const row of document.querySelectorAll(".wms-bar-row")) {
    row.classList.toggle("selected", row.dataset.member === view);
  }
}

function clearMemberView() {
  $("view-org").value = "";
  refetchSnapshot();
  updateViewBanner("");
  closeDrawer();
}

// ── Snapshot updates ────────────────────────────────────────────────────────

async function refetchSnapshot() {
  const view = $("view-org").value;
  const url = view
    ? `/api/snapshot?as=${encodeURIComponent(view)}&viewer=${encodeURIComponent(view)}`
    : "/api/snapshot";
  const response = await fetch(url);
  const data = await response.json();
  try {
    if (view) {
      orgSnapshotData = data;
      if (latest) renderDOM(latest);
      renderVisibilityFromOrg(data, view);
    } else {
      orgSnapshotData = null;
      renderDOM(data);
      renderVisibilityMatrixFromRun(data);
    }
  } catch (error) {
    console.error("render failed:", error);
  }
}

function subscribeEvents() {
  const source = new EventSource("/api/events");
  source.addEventListener("snapshot", (message) => {
    setConnection("live");
    latest = JSON.parse(message.data);
    try {
      renderDOM(latest);
      const view = $("view-org").value;
      if (view) {
        refreshOrgView(view);
      } else {
        renderVisibilityMatrixFromRun(latest);
      }
    } catch (error) {
      // A rendering bug must never silently blank the page: surface it and
      // keep the stream alive.
      console.error("render failed:", error);
    }
  });
  source.onerror = () => {
    setConnection("reconnecting");
    source.close();
    // A dropped or throttled stream gap is repaired with an authoritative
    // snapshot refetch; backend counters never rely on browser-facing state.
    refetchSnapshot().catch(() => {}).finally(() => setTimeout(subscribeEvents, 1500));
  };
}

function setConnection(state) {
  $("connection").textContent = state;
}

// Per-member view: fetch that member's own-node snapshot and overlay the
// payload holdings into the visibility matrix.
async function refreshOrgView(view) {
  const response = await fetch(
    `/api/snapshot?as=${encodeURIComponent(view)}&viewer=${encodeURIComponent(view)}`,
  );
  orgSnapshotData = await response.json();
  renderVisibilityFromOrg(orgSnapshotData, view);
}

function renderVisibilityFromOrg(orgData, view) {
  const held = (orgData.pdc_values || []).length;
  const height = orgData.chain_height ?? 0;
  const role = orgRoles[view] ?? "";
  $("visibility-body").replaceChildren(
    visibilityRow(view, role, height, held > 0 ? "holds payloads" : "commitments only"),
  );
}

// Full matrix from the public snapshot: org list + membership, no cleartext.
function renderVisibilityMatrixFromRun(state) {
  const matrixSig = JSON.stringify([state.orgs || [], state.chain_height ?? 0]);
  renderIfChanged("visibility", matrixSig, () => {
  const rows = (state.orgs || []).map((org) =>
    visibilityRow(
      org.id,
      org.role,
      state.chain_height ?? 0,
      org.evil ? "not a member" : org.member_of.length ? "holds payloads" : "commitments only",
      org.evil,
    ),
  );
  $("visibility-body").replaceChildren(...rows);
  });
}

function visibilityRow(id, role, height, pdc, evil = false) {
  const tr = document.createElement("tr");
  tr.dataset.member = id;
  if (evil) tr.className = "evil-row";
  const tdId = document.createElement("td");
  tdId.append(document.createTextNode(id));
  const tdRole = document.createElement("td");
  tdRole.append(document.createTextNode(role));
  const tdChain = document.createElement("td");
  tdChain.append(document.createTextNode(`height ${height}`));
  const tdPdc = document.createElement("td");
  const member = pdc.includes("holds payloads");
  tdPdc.className = member ? "good" : "bad";
  tdPdc.append(document.createTextNode(pdc));
  tr.append(tdId, tdRole, tdChain, tdPdc);
  return tr;
}
// ── DOM rendering (escapes by construction: DOM text nodes only) ────────────

// Section diff-renderer: rebuild a panel's children only when the data
// behind it changed — hover, selection and typed input survive ticks.
const renderSig = {};
function renderIfChanged(key, signature, rebuild) {
  if (renderSig[key] === signature) return;
  renderSig[key] = signature;
  rebuild();
}

// Advertised offers sidebar: one card per recent offer (newest last), with
// a Buy action for offers awaiting a human decision. The panel only
// re-renders when the offer set changes, and the typed quantity is kept
// per offer so a tick never snaps the user's input back to the maximum.
const typedQty = {};

function offersSignature(state) {
  return JSON.stringify(
    (state.offers || [])
      .filter((event) => event.kind === "offer" || event.kind === "purchase")
      .map((event) => [event.kind, event.tx_id, event.seller, event.buyer, event.quantity, event.sold, event.price_per_unit, event.note]),
  );
}
function offerCard(offer) {
  const card = document.createElement("div");
  card.className = "offer-card";
  const head = document.createElement("div");
  head.className = "head";
  const name = document.createElement("span");
  name.append(document.createTextNode(`${offer.product}`));
  const price = document.createElement("span");
  price.className = "price";
  price.append(document.createTextNode(`$${((offer.price_per_unit || 0) / 100).toFixed(2)}`));
  head.append(name, price);
  const parties = document.createElement("div");
  parties.className = "parties";
  const sold = offer.sold || 0;
  const label = offer.kind === "purchase"
    ? `${offer.quantity} units · ${offer.seller} → ${offer.buyer}`
    : `${sold}/${offer.quantity} of the lot's 500 units sold · ${offer.seller} → ${offer.buyer}`;
  parties.append(document.createTextNode(label));
  const note = document.createElement("div");
  note.className = "note";
  note.append(document.createTextNode(offer.note || ""));
  card.append(head, parties, note);
  const awaiting =
    offer.kind === "offer" &&
    (offer.note.includes("awaiting") || offer.note.includes("still on offer"));
  if (awaiting) {
    const remaining = offer.quantity - sold;
    const wanted = Math.min(typedQty[offer.tx_id] ?? remaining, remaining) || remaining;
    const row = document.createElement("div");
    row.style.cssText = "display:flex; gap:.4rem; margin-top:.4rem;";
    const qty = document.createElement("input");
    qty.type = "number";
    qty.min = "1";
    qty.max = String(remaining);
    qty.value = String(wanted);
    qty.style.cssText = "width:5.5em;";
    qty.title = `Units to buy (1–${remaining} available)`;
    qty.addEventListener("input", () => {
      const value = Number(qty.value);
      if (Number.isFinite(value)) typedQty[offer.tx_id] = value;
    });
    const buy = document.createElement("button");
    buy.textContent = "Buy";
    buy.title =
      "Submit a real PurchaseOrder through the viewing member's own node — it relays, is admitted and commits like any transaction";
    buy.addEventListener("click", () => {
      const typed = Number(qty.value) || remaining;
      completePurchase(offer, Math.min(typed, remaining));
    });
    row.append(qty, buy);
    card.append(row);
  }
  return card;
}

function renderOffers(state) {
  const offers = (state.offers || []).filter((event) => event.kind === "offer");
  const purchases = (state.offers || []).filter((event) => event.kind === "purchase");
  const pending = offers.filter(
    (offer) => offer.note.includes("awaiting") || offer.note.includes("still on offer"),
  );
  const pendingList = $("offers-pending");
  pendingList.replaceChildren();
  for (const offer of pending.slice(-6).reverse()) {
    pendingList.append(offerCard(offer));
  }
  if (!pending.length) {
    pendingList.append(drawerParagraph("Nothing pending — every offer is filled."));
  }
  const executed = [
    ...purchases,
    ...offers.filter((offer) => !pending.includes(offer)),
  ].slice(-8).reverse();
  const executedList = $("offers-executed");
  executedList.replaceChildren();
  for (const entry of executed) {
    executedList.append(offerCard(entry));
  }
  if (!executed.length) {
    executedList.append(drawerParagraph("No executed purchases yet — start the run."));
  }
}

async function completePurchase(offer, quantity) {
  const buyer = $("view-org").value || "pharmacy-1";
  const response = await fetch("/api/purchase", {
    method: "POST",
    headers: { "content-type": "application/json", "x-glass-auth": token },
    body: JSON.stringify({ offer_tx_id: offer.tx_id, buyer, quantity }),
  });
  const result = await response.json();
  $("params-status").textContent = response.ok
    ? `Purchase submitted (${result.tx}) — commits with the next block`
    : `rejected: ${result.error}`;
}

function rows0(state) {
  const metrics = state.metrics || {};
  return [
    metrics.submitted || 0,
    metrics.rejected || 0,
    metrics.lots || 0,
    metrics.blocks || 0,
    state.chain_height || 0,
    metrics.tx_per_sec || 0,
    metrics.elapsed_s || 0,
    metrics.last_commit_ms || 0,
    metrics.commit_p50_ms || 0,
    metrics.commit_p95_ms || 0,
    metrics.pool_count || 0,
    metrics.pool_bytes || 0,
    (state.pdc || []).reduce((n, entry) => n + entry.commitments.length, 0),
  ];
}

function renderDOM(state) {
  if (!state) return;
  const status = state.status || "idle";
  $("run-status").textContent = status;
  $("run-status").dataset.live = status;

  // Offers lists (pending | executed) in the left org sidebar.
  const offersSig = JSON.stringify(
    (state.offers || []).map((event) => [
      event.kind, event.tx_id, event.seller, event.buyer,
      event.quantity, event.sold, event.price_per_unit, event.note,
    ]),
  );
  renderIfChanged("offers", offersSig, () => renderOffers(state));

  // Metric cards: text updates only when a number changed.
  const metricSig = JSON.stringify(rows0(state));
  renderIfChanged("metrics", metricSig, () => {
    const metrics = state.metrics || {};
    const rows = [
      ["Submitted", metrics.submitted || 0, "Transactions accepted into the pending pool — admission, not finality"],
      ["Rejected", metrics.rejected || 0, "Admission rejections: evil attempts + any invalid transaction"],
      ["Lots", metrics.lots || 0, "Synthetic lots produced since the run started"],
      ["Blocks mined", metrics.blocks || 0, "Blocks committed by the dev-PoW driver (one per round)"],
      ["Chain height", state.chain_height || 0, "Committed blocks including genesis and the setup block"],
      ["Throughput (tx/s)", metrics.tx_per_sec || 0, "Average committed-side submissions per second"],
      ["Elapsed (s)", metrics.elapsed_s || 0, "Seconds since the run started"],
      ["Last commit (ms)", metrics.last_commit_ms || 0, "Dev-PoW block mining time (not BFT finality)"],
      ["Commit p50 (ms)", metrics.commit_p50_ms || 0, "Median block commit over the last 64 rounds"],
      ["Commit p95 (ms)", metrics.commit_p95_ms || 0, "95th percentile commit — outliers included, on purpose"],
      ["Pending pool", metrics.pool_count || 0, "Transactions waiting in the miner's pool right now"],
      ["Pool bytes", metrics.pool_bytes || 0, "Serialized size of the pending pool"],
      ["PDC commitments", (state.pdc || []).reduce((n, entry) => n + entry.commitments.length, 0), "On-chain sha256 commitments of private pricing payloads"],
    ];
    $("metrics").replaceChildren(
      ...rows.map(([label, value]) => {
        const box = document.createElement("div");
        const dt = document.createElement("dt");
        dt.append(document.createTextNode(label));
        const dd = document.createElement("dd");
        dd.append(document.createTextNode(String(value)));
        box.append(dt, dd);
        box.title = label;
        return box;
      }),
    );
  });

  // Lots: rebuild only when a row's visible fields changed.
  const lotsSig = JSON.stringify(
    (state.lots || []).map((lot) => [
      lot.lot_ref, lot.status, lot.manufacturer, lot.trust_score,
      (lot.chain || []).map((step) => `${step.event_type}${step.custodian}${step.block}`),
    ]),
  );
  renderIfChanged("lots", lotsSig, () => {
    $("lots-body").replaceChildren(
      ...(state.lots || []).map((lot) => {
        const tr = document.createElement("tr");
        tr.className = "clickable";
        tr.title = `${lot.lot_ref} — click for its full history, PDC info and graph path`;
        tr.addEventListener("click", () => openLotDrawer(lot));
        const tdLot = document.createElement("td");
        tdLot.append(document.createTextNode(lot.lot_ref));
        const tdStatus = document.createElement("td");
        tdStatus.append(document.createTextNode(lot.status));
        tdStatus.style.color = lot.status === "complete" ? "var(--accent)" : "var(--muted)";
        const tdMaker = document.createElement("td");
        tdMaker.append(document.createTextNode(lot.manufacturer || ""));
        const tdTrust = document.createElement("td");
        const score = lot.trust_score ?? 0;
        tdTrust.append(document.createTextNode(`${score}/100`));
        tdTrust.className = score >= 80 ? "good" : score > 0 ? "bad" : "";
        const tdChain = document.createElement("td");
        const steps = (lot.chain || [])
          .map((item) => `${item.event_type} → ${item.custodian} @${item.block}`)
          .join("  |  ");
        tdChain.append(document.createTextNode(steps || "—"));
        tr.append(tdLot, tdStatus, tdMaker, tdTrust, tdChain);
        return tr;
      }),
    );
  });
  // Stock-level bars, value-share donut, and the inventory directory —
  // the reference layout: one chart per panel, a directory table, no cards.
  const wmsSig = JSON.stringify([state.wms || [], state.inventory || {}]);
  renderIfChanged("wms", wmsSig, () => {
    const rows = state.wms || [];
    const wmsTotal = Math.max(1, ...rows.map((row) => row.units_here));
    const totalValue = Math.max(
      1,
      rows.reduce((n, row) => n + (row.stock_value_minor || 0), 0),
    );

    $("wms-bars").replaceChildren(
      ...rows.map((row) => {
        const sellable = Number((state.inventory || {})[row.company] ?? 0);
        const low = sellable < 100;
        const line = document.createElement("div");
        line.className = "wms-bar-row";
        line.dataset.member = row.company;
        line.addEventListener("click", () => openMemberDrawer(row.company));
        line.title = `${row.company}: ${row.units_here} on-hand · ${sellable} sellable — click to manage`;
        const name = document.createElement("span");
        name.className = "name";
        name.append(document.createTextNode(row.company));
        const track = document.createElement("span");
        track.className = "track";
        const fill = document.createElement("span");
        fill.className = "fill";
        fill.style.width = `${Math.round((row.units_here / wmsTotal) * 100)}%`;
        track.append(fill);
        if (low) {
          const flag = document.createElement("span");
          flag.className = "flag";
          track.append(flag);
        }
        line.append(name, track);
        return line;
      }),
    );

    // Donut via conic-gradient stops; small shares fold into "Others".
    const sorted = [...rows].sort(
      (left, right) => (right.stock_value_minor || 0) - (left.stock_value_minor || 0),
    );
    let cursor = 0;
    const stops = [];
    const legendItems = [];
    sorted.forEach((row, index) => {
      const share = (row.stock_value_minor || 0) / totalValue;
      if (share > 0.04 || index < 5) {
        const from = (cursor / totalValue) * 100;
        cursor += row.stock_value_minor || 0;
        const to = (cursor / totalValue) * 100;
        const color = SEGMENT_COLORS[index % SEGMENT_COLORS.length];
        stops.push(`${color} ${from}% ${to}%`);
        legendItems.push({ name: row.company, color, pct: share });
      } else {
        cursor += row.stock_value_minor || 0;
      }
    });
    const othersPct = 1 - cursor / totalValue;
    if (othersPct > 0.005) {
      stops.push("#8a8f98");
      legendItems.push({ name: "Others", color: "#8a8f98", pct: othersPct });
    }
    $("wms-donut").style.background = `conic-gradient(${stops.join(", ")})`;
    $("wms-legend").replaceChildren(
      ...legendItems.map((item) => {
        const li = document.createElement("li");
        const dot = document.createElement("span");
        dot.className = "dot";
        dot.style.background = item.color;
        const name = document.createElement("span");
        name.append(document.createTextNode(item.name));
        const pct = document.createElement("span");
        pct.className = "pct";
        pct.append(document.createTextNode(`${Math.round(item.pct * 100)}%`));
        li.append(dot, name, pct);
        return li;
      }),
    );

    $("wms-directory-body").replaceChildren(
      ...rows.map((row) => {
        const sellable = Number((state.inventory || {})[row.company] ?? 0);
        const tr = document.createElement("tr");
        tr.className = "clickable";
        tr.dataset.member = row.company;
        for (const value of [
          row.company,
          row.role,
          String(row.lots_here),
          String(row.units_here),
          `$${((row.stock_value_minor || 0) / 100).toLocaleString(undefined, { maximumFractionDigits: 0 })}`,
          String(row.sold_units || 0),
        ]) {
          const td = document.createElement("td");
          td.append(document.createTextNode(value));
          tr.append(td);
        }
        tr.addEventListener("click", () => openMemberDrawer(row.company));
        return tr;
      }),
    );
  });

  // Purchases (executed trade): auto + manual, from the offers ledger.
  const purchasesSig = JSON.stringify(
    (state.offers || []).filter((event) => event.kind === "purchase"),
  );
  renderIfChanged("purchases", purchasesSig, () => {
    const purchases = (state.offers || [])
      .filter((event) => event.kind === "purchase")
      .slice(-8)
      .reverse();
    $("purchases-body").replaceChildren(
      ...purchases.map((order) => {
        const tr = document.createElement("tr");
        for (const [value] of [
          [order.product],
          [order.seller],
          [order.buyer],
          [String(order.quantity)],
          [`$${((order.price_per_unit || 0) / 100).toFixed(2)}`],
          [order.note || ""],
        ]) {
          const td = document.createElement("td");
          td.append(document.createTextNode(value));
          tr.append(td);
        }
        return tr;
      }),
    );
  });

  // Certifications.
  const certsSig = JSON.stringify(state.certs || []);
  renderIfChanged("certs", certsSig, () => {
    $("certs-body").replaceChildren(
      ...(state.certs || []).map((cert) => {
        const tr = document.createElement("tr");
        tr.className = "clickable";
        tr.title = `${cert.record_id} — click for its details and the related custody chain`;
        tr.addEventListener("click", () => openCertDrawer(cert));
        for (const key of ["record_id", "schema", "lot_ref", "issuer", "status"]) {
          const td = document.createElement("td");
          td.append(document.createTextNode(cert[key] ?? ""));
          tr.append(td);
        }
        return tr;
      }),
    );
  });

  // Zero-trust rows (clickable, with expandable explanation rows).
  const securitySig = JSON.stringify(
    (state.security || []).map((event) => [
      event.actor, event.action, event.outcome, event.detail, event.explanation,
    ]),
  );
  renderIfChanged("security", securitySig, () => {
    $("security-body").replaceChildren(
      ...(state.security || []).map((event) => {
        const tr = document.createElement("tr");
        tr.className = "evil-row clickable";
        tr.title = "Click to see why";
        for (const key of ["actor", "action", "outcome", "detail"]) {
          const td = document.createElement("td");
          const value = String(event[key] ?? "");
          td.append(document.createTextNode(value));
          if (key === "outcome") {
            const rejected =
              value.includes("rejected") || value.includes("fail-closed");
            td.className = value.includes("admitted") ? "amber" : rejected ? "good" : "bad";
          }
          tr.append(td);
        }
        const explanation = document.createElement("tr");
        explanation.className = "explanation-row hidden";
        const td = document.createElement("td");
        td.colSpan = 4;
        td.append(document.createTextNode(String(event.explanation || "").replace(/\s+/g, " ").trim()));
        explanation.append(td);
        tr.addEventListener("click", () => {
          explanation.classList.toggle("hidden");
          openDrawerForEvent(event);
        });
        return [tr, explanation];
      }).flat(),
    );
  });

  const equivocations = $("equivocations");
  const proofs = state.equivocations || [];
  if (proofs.length) {
    equivocations.classList.remove("hidden");
    equivocations.replaceChildren(
      ...proofs.map((note) => {
        const li = document.createElement("li");
        li.append(document.createTextNode(note));
        return li;
      }),
    );
  }

  const feedSig = JSON.stringify(state.feed || []);
  renderIfChanged("feed", feedSig, () => {
    $("feed").replaceChildren(
      ...(state.feed || []).map((item) => {
        const li = document.createElement("li");
        li.append(document.createTextNode(`${item.label} (h ${item.height})`));
        return li;
      }),
    );
  });
}

// ── Scene: one node per company, edges weighted by traffic ──────────────────

// Layout: one lane per role; companies spread evenly along the lane. User
// drags override positions (per node id).
function nodePoints() {
  const orgs = (latest && latest.orgs) || [];
  const lanes = {};
  for (const org of orgs) {
    if (!(org.role in lanes)) lanes[org.role] = [];
    lanes[org.role].push(org);
  }
  const laneX = { manufacturer: 0.12, distributor: 0.36, logistics: 0.6, pharmacy: 0.85 };
  const points = [];
  for (const [role, members] of Object.entries(lanes)) {
    const x = laneX[role] ?? 0.5;
    members.forEach((org, index) => {
      const base = [xLane(role), 0.12 + 0.76 * spreadFor(members.length, index)];
      const off = dragOffsets[org.id];
      points.push({
        id: org.id,
        role: org.role,
        evil: org.evil,
        x: off ? off.x : base[0],
        y: off ? off.y : base[1],
      });
    });
  }
  return points;
}

function xLane(role) {
  return { manufacturer: 0.12, distributor: 0.36, logistics: 0.6, pharmacy: 0.85 }[role] ?? 0.5;
}

function spreadFor(count, index) {
  return count <= 1 ? 0.5 : index / (count - 1);
}

function edgeList() {
  return (latest && latest.edges) || [];
}

// One moving dot per recent transaction, flowing along its own edge at a
// phase offset by its index; self-edges (manufacture) stay put as pulses.
let txDots = [];

function txPoints(when) {
  const status = latest && latest.status;
  if (status !== "running" && status !== "rebuilding") {
    // The run is stopped: nothing is in flight, so nothing flows.
    txDots = [];
    return [];
  }
  const nodes = nodePoints();
  const byId = Object.fromEntries(nodes.map((node) => [node.id, node]));
  const txs = (latest && latest.transactions) || [];
  txDots = [];
  const dots = [];
  // Reduced motion: a slower, capped flow; a paused screen keeps the dots
  // where they are (no travel advance).
  const period = document.documentElement.dataset.motion === "still" ? Infinity : 2400;
  const cap = document.documentElement.dataset.motion === "still" ? 0 : 2;
  let taken = 0;
  txs.forEach((tx, index) => {
    const from = byId[tx.from];
    const to = byId[tx.to];
    if (!from || !to) return;
    if (tx.from !== tx.to) {
      if (taken >= cap) return;
      taken += 1;
    }
    const travel = tx.from === tx.to ? 0 : (when / period + index * 0.13) % 1;
    const dot = {
      x: from.x + (to.x - from.x) * travel,
      y: from.y + (to.y - from.y) * travel,
      tx,
    };
    txDots.push(dot);
    dots.push(dot);
  });
  return dots;
}

// ── Canvas2D renderer (the baseline) ────────────────────────────────────────

function nodeRadius() {
  const count = nodePoints().length;
  return count > 16 ? 11 : count > 10 ? 13 : 16;
}

function drawCanvas2D(when) {
  const width = canvas.width;
  const height = canvas.height;
  ctx2d.fillStyle = "#0d1b23";
  ctx2d.fillRect(0, 0, width, height);
  ctx2d.font = "11px system-ui";
  ctx2d.textAlign = "center";

  const nodes = nodePoints();
  const byId = Object.fromEntries(nodes.map((node) => [node.id, node]));
  // Edges: width grows with the traffic that crossed it. The highlighted
  // lot's custody path is drawn in accent while its drawer is open.
  for (const edge of edgeList()) {
    const from = byId[edge.from];
    const to = byId[edge.to];
    if (!from || !to) continue;
    const highlighted =
      highlightPath &&
      highlightPath.includes(edge.from) &&
      highlightPath.includes(edge.to) &&
      Math.abs(
        highlightPath.indexOf(edge.from) - highlightPath.indexOf(edge.to),
      ) === 1;
    ctx2d.strokeStyle = highlighted ? "rgb(110, 121, 228)" : "#2a4a59";
    ctx2d.lineWidth = highlighted
      ? 4
      : 1 + Math.min(5, Math.sqrt(edge.count) / 3);
    ctx2d.beginPath();
    ctx2d.moveTo(from.x * width, from.y * height);
    ctx2d.lineTo(to.x * width, to.y * height);
    ctx2d.stroke();
  }
  // Moving dots: one per recent transaction, colored by kind.
  const KIND_COLORS = {
    lot: [227, 232, 68],
    custody: [94, 214, 178],
    offer: [122, 168, 255],
    purchase: [255, 158, 100],
    attestation: [235, 219, 138],
  };
  for (const dot of txPoints(when)) {
    const color = KIND_COLORS[dot.tx.kind] || [227, 232, 68];
    ctx2d.fillStyle = `rgba(${color.join(",")}, 0.95)`;
    ctx2d.beginPath();
    ctx2d.arc(dot.x * width, dot.y * height, 4, 0, Math.PI * 2);
    ctx2d.fill();
    if (dot.tx === hoverTx) {
      ctx2d.strokeStyle = "#ffffff";
      ctx2d.lineWidth = 2;
      ctx2d.beginPath();
      ctx2d.arc(dot.x * width, dot.y * height, 7, 0, Math.PI * 2);
      ctx2d.stroke();
    }
  }
  // Node discs + labels.
  const radius = nodeRadius();
  for (const node of nodes) {
    const color = node.evil ? EVIL_COLOR : ROLE_COLORS[node.role] || [128, 128, 128];
    const cx = node.x * width;
    const cy = node.y * height;
    ctx2d.fillStyle = `rgb(${color.join(",")})`;
    ctx2d.beginPath();
    ctx2d.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx2d.fill();
    if (node.id === hoverId) {
      ctx2d.strokeStyle = "#ffffff";
      ctx2d.lineWidth = 2;
      ctx2d.beginPath();
      ctx2d.arc(cx, cy, radius + 4, 0, Math.PI * 2);
      ctx2d.stroke();
    }
    ctx2d.fillStyle = node.evil ? "#f3b3b3" : "#dfeae7";
    ctx2d.fillText(node.id, cx, cy + radius + 12);
  }
  // Hover tooltip: node identity or transaction detail.
  const hoverLabel = hoverTx
    ? `${hoverTx.kind}: ${hoverTx.label} (tx ${hoverTx.id.slice(0, 8)}…, h ${hoverTx.height}) — click for details`
    : hoverId
      ? (() => {
          const node = nodes.find((candidate) => candidate.id === hoverId);
          return node ? `${node.id} — ${node.role}${node.evil ? " (EVIL)" : ""}` : null;
        })()
      : null;
  if (hoverLabel) {
    const hoverPoint = hoverTx
      ? txDots.find((dot) => dot.tx === hoverTx)
      : (() => {
          const node = nodes.find((candidate) => candidate.id === hoverId);
          return node ? { x: node.x, y: node.y } : null;
        })();
    if (hoverPoint) {
      const cx = hoverPoint.x * width;
      const cy = hoverPoint.y * height - (hoverTx ? 14 : radius + 12);
      const boxWidth = ctx2d.measureText(hoverLabel).width + 16;
      ctx2d.fillStyle = "rgba(13, 27, 35, 0.92)";
      ctx2d.fillRect(cx - boxWidth / 2, cy - 20, boxWidth, 24);
      ctx2d.fillStyle = "#dfeae7";
      ctx2d.fillText(hoverLabel, cx, cy - 4);
    }
  }
}

// ── Graph interactivity: click = view as that member; drag = rearrange ──────

function canvasPoint(event) {
  const rect = canvas.getBoundingClientRect();
  return {
    x: (event.clientX - rect.left) / rect.width,
    y: (event.clientY - rect.top) / rect.height,
  };
}

function nodeAt(point) {
  let best = null;
  let bestDistance = Infinity;
  for (const node of nodePoints()) {
    const distance = Math.hypot(node.x - point.x, node.y - point.y);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = node;
    }
  }
  return bestDistance < 0.06 ? best : null;
}

function txAt(point) {
  let best = null;
  let bestDistance = Infinity;
  for (const dot of txDots) {
    const distance = Math.hypot(dot.x - point.x, dot.y - point.y);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = dot;
    }
  }
  return bestDistance < 0.035 ? best : null;
}

function attachGraphInteractivity(canvasEl) {
  canvasEl.addEventListener("mousemove", (event) => {
    const point = canvasPoint(event);
    if (armedNode && event.buttons & 1) {
      dragging = armedNode;
      dragMoved = true;
      dragOffsets[dragging] = {
        x: Math.min(1, Math.max(0, point.x)),
        y: Math.min(1, Math.max(0, point.y)),
      };
      return;
    }
    const node = nodeAt(point);
    hoverId = node ? node.id : null;
    const dot = txAt(point);
    hoverTx = dot && !node ? dot.tx : null;
    canvasEl.style.cursor = dot || node ? "pointer" : "default";
  });
  let armedNode = null;
  let dragMoved = false;
  canvasEl.addEventListener("mousedown", (event) => {
    const node = nodeAt(canvasPoint(event));
    armedNode = node ? node.id : null;
    dragMoved = false;
    if (node) event.preventDefault();
  });
  canvasEl.addEventListener("mouseup", (event) => {
    const point = canvasPoint(event);
    if (dragMoved) {
      armedNode = null;
      return;
    }
    // A node wins over a passing transaction dot: clicking a company must
    // open the member drawer even when dots flow across it.
    if (armedNode) {
      // Click a member: inspect its drawer WITHOUT switching the global
      // view — the drawer is viewer-scoped (`?as=X&viewer=V`), so it shows
      // what the current viewer can see of that member.
      openMemberDrawer(armedNode);
      armedNode = null;
    }
  });
  canvasEl.addEventListener("mouseleave", () => {
    hoverId = null;
    hoverTx = null;
    dragging = null;
  });
}

// ── Member drawer: what this member genuinely sees, with explanations ───────

function drawerShell(org, role, evil, member) {
  const content = $("drawer-content");
  content.replaceChildren();
  const h3 = document.createElement("h3");
  h3.style.fontSize = "1.15rem";
  h3.append(document.createTextNode(org));
  const badges = document.createElement("p");
  const roleBadge = document.createElement("span");
  roleBadge.className = "badge " + (evil ? "evil" : member ? "member" : "outsider");
  roleBadge.append(document.createTextNode(role || "member"));
  const stateBadge = document.createElement("span");
  stateBadge.className = "badge " + (evil ? "evil" : member ? "member" : "outsider");
  stateBadge.append(
    document.createTextNode(evil ? "EVIL — not a member" : member ? "PDC member" : "no PDC access"),
  );
  badges.append(roleBadge, " ", stateBadge);
  content.append(h3, badges);
  return content;
}



// Kind-aware one-line summary of a private payload's terms.
function describeTerms(terms) {
  if (terms.kind === "process") {
    return `${terms.steps.join(" → ")} · batch ${terms.batch_record} · ${terms.gmp_line} · shift ${terms.operator_shift}`;
  }
  if (terms.kind === "intake") {
    return `checks: ${terms.checks.join(", ")} · quarantine ${terms.quarantine_hours} h`;
  }
  if (terms.kind === "temperature_log") {
    return `${terms.sensor} · ${terms.min_c}–${terms.max_c} °C · ${terms.samples_per_hour}/h · ${terms.excursions} excursions`;
  }
  if (terms.kind === "storage") {
    return `${terms.warehouse} · ${terms.temp_range} · humidity ${terms.humidity} · retain ${terms.retention_days}d · ${terms.lot_release}`;
  }
  if (terms.kind === "transit") {
    return `${terms.route} · ${terms.transit_hours} h transit · ${terms.cold_chain} cold chain · ${terms.delivery_window}`;
  }
  if (terms.kind === "certification_evidence") {
    return `${terms.findings} · ${terms.samples_tested} samples · lab ${terms.lab_reference} · next audit ${terms.next_audit}`;
  }
  if (terms.kind === "regulator_notes") {
    return `${terms.inspection} · finding: ${terms.finding} · follow-up ${terms.follow_up_required ? "required" : "not required"} · ${terms.inspector}`;
  }
  return `member $${((terms.member_price_per_unit || 0) / 100).toFixed(2)} · list $${((terms.list_price_per_unit || 0) / 100).toFixed(2)} · ${terms.quantity} ${terms.currency} · ${terms.payment_terms}`;
}

function drawerParagraph(text) {
  const p = document.createElement("p");
  p.className = "hint";
  p.append(document.createTextNode(text));
  return p;
}

function showDrawer() {
  $("member-drawer").classList.add("open");
  $("drawer-backdrop").classList.add("open");
}

// Drag the drawer's left edge to resize its width.
function attachDrawerResizer() {
  const drawer = $("member-drawer");
  const handle = document.createElement("div");
  handle.className = "drawer-resize";
  handle.title = "Drag to resize the drawer";
  drawer.append(handle);
  let startX = 0;
  let startW = 0;
  handle.addEventListener("mousedown", (event) => {
    startX = event.clientX;
    startW = drawer.getBoundingClientRect().width;
    event.preventDefault();
    const move = (moveEvent) => {
      const width = Math.min(
        window.innerWidth * 0.9,
        Math.max(20, startW + (startX - moveEvent.clientX)),
      );
      drawer.style.width = `${Math.round(width)}px`;
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
}

// Wrap a drawer table in a bounded scroll container.
function scrollWrap(table) {
  const wrap = document.createElement("div");
  wrap.className = "drawer-scroll";
  // Every drawer table is resizable: a thin drag handle above the table
  // adjusts its scroll height.
  const handle = document.createElement("div");
  handle.className = "resize-handle";
  handle.title = "Drag to resize this table";
  handle.addEventListener("mousedown", (event) => {
    const target = wrap;
    const startY = event.clientY;
    const startH = target.getBoundingClientRect().height;
    event.preventDefault();
    const move = (moveEvent) => {
      const height = Math.min(
        window.innerHeight * 0.8,
        Math.max(6, startH + (moveEvent.clientY - startY)),
      );
      target.style.maxHeight = `${Math.round(height)}px`;
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  wrap.append(handle, table);
  return wrap;
}

function closeDrawer() {
  $("member-drawer").classList.remove("open");
  $("drawer-backdrop").classList.remove("open");
  highlightPath = null;
}

async function openMemberDrawer(org) {
  const admin = currentViewOrg() === "admin";
  const orgs = (latest && latest.orgs) || [];
  const view = orgs.find((candidate) => candidate.id === org);
  const evil = view ? view.evil : false;
  const member = !evil && view ? view.member_of.length > 0 : false;
  const content = drawerShell(org, view ? view.role : "", evil, member);
  const viewer = currentViewOrg();
  const viewerParam = viewer ? `&viewer=${encodeURIComponent(viewer)}` : "";
  const response = await fetch(`/api/snapshot?as=${encodeURIComponent(org)}${viewerParam}`);
  const data = await response.json();
  let adminData = null;
  if (admin) {
    // Admin (demo): the payload inspection reads the privileged lens —
    // every payload's cleartext — and cash comes from the global map.
    const adminResponse = await fetch("/api/snapshot?as=admin");
    adminData = await adminResponse.json();
    data.pdc_values = adminData.pdc_values || [];
    data.cash = (adminData.cash_all || {})[org] ?? data.cash;
  }
  if (data.error) {
    content.append(drawerParagraph(data.error));
  } else {
    content.append(
      drawerParagraph(
        `Read from ${org}'s own node — not the leader's view filtered. Its chain height is ${data.chain_height}, the same as every synced member.`,
      ),
    );
    // Warehouse + cash + counterparty offers, from the member's own view.
    const h4b = document.createElement("h4");
    h4b.append(document.createTextNode("Warehouse, cash and counterparties"));
    content.append(h4b);
    if (data.stock) {
      content.append(drawerParagraph(
        `Stock on hand (committed custody): ${data.stock.lots_here} lot(s) · ${data.stock.units_here} units — receipts ${data.stock.received_units} · dispatches ${data.stock.dispatched_units}`,
      ));
    } else {
      content.append(drawerParagraph("No stock on hand yet."));
    }
    if (!data.private_visible) {
      content.append(drawerParagraph(
        "This member's cash and trade bookkeeping are private — visible only to the member itself, the regulator, and Admin (demo). Impersonate one of those to inspect it.",
      ));
    }

    // Inventory by SKU and lot: every lot whose committed custodian is this
    // member, one row each.
    const heldLots = ((latest && latest.lots) || []).filter(
      (lot) => lot.chain.length && lot.chain[lot.chain.length - 1].custodian === org,
    );
    if (heldLots.length) {
      const invTable = document.createElement("table");
      invTable.className = "drawer-orders";
      invTable.innerHTML =
        "<thead><tr><th>SKU</th><th>Lot</th><th>Units</th><th>Acquired at</th><th>Source</th></tr></thead>";
      const invBody = document.createElement("tbody");
      for (const lot of heldLots.slice(-8).reverse()) {
        const tr = document.createElement("tr");
        for (const [value] of [
          [lot.product_hint ?? "SKU-DEMO"],
          [lot.lot_ref],
          ["500"],
          [`block ${lot.chain[lot.chain.length - 1].block}`],
          [lot.manufacturer || "—"],
        ]) {
          const td = document.createElement("td");
          td.append(document.createTextNode(value));
          tr.append(td);
        }
        invBody.append(tr);
      }
      invTable.append(invBody);
      content.append(scrollWrap(invTable));
    }
    content.append(drawerParagraph(
      data.cash == null
        ? "Cash: not visible in this view (member-private; visible to the member, the regulator and Admin)."
        : `Cash (demo bookkeeping — not an on-chain concept): $${((data.cash ?? 0) / 100).toLocaleString()}`,
    ));
    const sellsList = data.sell_offers || [];
    const buysList = data.purchases || [];
    const tradeTable = document.createElement("table");
    tradeTable.className = "pdc-table";
    tradeTable.innerHTML =
      "<thead><tr><th>Type</th><th>Product</th><th>Units</th><th>Price</th><th>Note</th></tr></thead>";
    const tradeBody = document.createElement("tbody");
    const tradeRows = [
      ...sellsList.slice(-4).reverse().map((sell) => ({
        kind: "Sell",
        product: sell.product,
        units: `${sell.quantity - (sell.sold || 0)} remaining of ${sell.quantity}`,
        price: `$${((sell.price_per_unit || 0) / 100).toFixed(2)}`,
        note: sell.note || "",
        pending: sell.note.includes("awaiting") || sell.note.includes("still on offer"),
      })),
      ...buysList.slice(-4).reverse().map((buy) => ({
        kind: "Buy",
        product: buy.product,
        units: `${buy.quantity}`,
        price: `$${((buy.price_per_unit || 0) / 100).toFixed(2)}`,
        note: `from ${buy.seller}`,
        pending: false,
      })),
    ];
    for (const row of tradeRows) {
      const tr = document.createElement("tr");
      for (const text of [row.kind, row.product, row.units, row.price, row.note]) {
        const td = document.createElement("td");
        td.append(document.createTextNode(text));
        tr.append(td);
      }
      const tdStatus = document.createElement("td");
      const badge = document.createElement("span");
      badge.className = `badge health ${row.pending ? "health-low" : "health-good"}`;
      badge.append(document.createTextNode(row.pending ? "Pending" : "Concluded"));
      tdStatus.append(badge);
      tr.append(tdStatus);
      tradeBody.append(tr);
    }
    tradeTable.append(tradeBody);
    if (!tradeRows.length) {
      content.append(drawerParagraph("No advertised offers or purchases yet."));
    } else {
      content.append(tradeTable);
    }

    // PDC inspection — the LAST section, as a table: one row per payload.
    const commitments = ((latest && latest.pdc) || [])
      .flatMap((entry) => entry.commitments || []);
    const pdcRows = admin
      ? (adminData?.pdc_values || [])
      : (data.pdc_values || []);
    const h4pdc = document.createElement("h4");
    h4pdc.append(document.createTextNode("PDC inspection — collection `pricing`"));
    content.append(h4pdc);
    const readable = pdcRows.filter((value) => value.payload).length;
    content.append(drawerParagraph(
      `${pdcRows.length} payload(s) owned by ${org} · readable by ${
        viewer === "admin" ? "Admin (demo)" : viewer || "the public lens"
      }: ${readable}`,
    ));
    if (pdcRows.length) {
      const pdcTable = document.createElement("table");
      pdcTable.className = "pdc-table";
      pdcTable.innerHTML =
        "<thead><tr><th>Kind</th><th>Lot</th><th>Terms</th><th>Commitment</th></tr></thead>";
      const pdcBody = document.createElement("tbody");
      for (const value of pdcRows) {
        const tr = document.createElement("tr");
        const kind = value.kind ?? "—";
        const lot = value.lot ? String(value.lot) : "—";
        const summary = value.summary || "—";
        for (const text of [kind, lot, summary, `${value.commitment.slice(0, 12)}…`]) {
          const td = document.createElement("td");
          td.append(document.createTextNode(text));
          tr.append(td);
        }
        pdcBody.append(tr);
      }
      pdcTable.append(pdcBody);
      content.append(scrollWrap(pdcTable));
      content.append(drawerParagraph(
        "Every row above is this member's own payload — held in its node and readable here. Other members' payloads were never part of this view.",
      ));
    } else {
      content.append(drawerParagraph(
        "No private payloads authored by this member yet.",
      ));
    }
  }
  showDrawer();
}

// Graph highlight: a lot's custody path drawn in accent for as long as
// its drawer stays open.
let highlightPath = null;

function openLotDrawer(lot) {
  const content = drawerShell(lot.lot_ref, `lot · made by ${lot.manufacturer}`, false, true);
  content.append(drawerParagraph(
    `Trust score ${lot.trust_score}/100 at the origin registration · status ${lot.status}`,
  ));
  const stages = lot.chain || [];
  const list = document.createElement("ul");
  if (stages.length) {
    for (const stage of stages) {
      const li = document.createElement("li");
      li.append(document.createTextNode(
        `${stage.event_type} → ${stage.custodian} · block ${stage.block}`,
      ));
      list.append(li);
    }
    content.append(list);
    // The custody path to highlight on the live graph while this drawer
    // stays open.
    highlightPath = stages.map((stage) => stage.custodian);
  } else {
    content.append(drawerParagraph("No committed custody events yet."));
  }
  const certsFor = ((latest && latest.certs) || []).filter(
    (cert) => cert.lot_ref === lot.lot_ref,
  );
  const h4 = document.createElement("h4");
  h4.append(document.createTextNode("Certifications and audits"));
  content.append(h4);
  const certList = document.createElement("ul");
  const certRows = certsFor.slice(-4);
  for (const cert of certRows) {
    const li = document.createElement("li");
    li.append(document.createTextNode(`${cert.schema} — ${cert.issuer} (${cert.status})`));
    list.append(li);
  }
  if (!certRows.length) {
    list.append(drawerParagraph("No certification or audit record for this lot yet."));
  }
  content.append(list);
  // PDC info: EVERY payload this lot carries (pricing, storage, transit —
  // up to three, each authored by a different org), each row showing its
  // commitment hash, author and access badge for the current viewer.
  const seq = Number(lot.lot_ref.replace("LOT-", "")) || 0;
  const view = currentViewOrg();
  const h4b = document.createElement("h4");
  h4b.append(document.createTextNode("PDC info — collection `pricing`"));
  content.append(h4b);
  if (!view) {
    content.append(drawerParagraph(
      "Public view: the chain holds only sha256 commitments — no cleartext is readable. Use the member selector to inspect as a member.",
    ));
  } else if (view === "admin") {
    content.append(drawerParagraph(
      "Admin (demo): every payload's cleartext is readable.",
    ));
  } else if (lot.manufacturer === view) {
    content.append(drawerParagraph(
      `Your own terms as ${view} are readable; the other members' payloads for this lot stay commitments.`,
    ));
  } else {
    content.append(drawerParagraph(
      `${lot.manufacturer}'s pricing terms are not readable by ${view} — visible to its author and the regulator.`,
    ));
  }
  const pdcTable = document.createElement("table");
  pdcTable.className = "pdc-table";
  pdcTable.innerHTML =
    "<thead><tr><th>Kind</th><th>Author</th><th>Terms</th><th>Commitment</th><th>Access</th></tr></thead>";
  const pdcBody = document.createElement("tbody");
  const KIND_LABEL = {
    pricing: "Pricing terms",
    storage: "Storage terms",
    transit: "Transit terms",
    process: "Process terms",
    intake: "Intake checklist",
    temperature_log: "Temperature log",
    certification_evidence: "Certification evidence",
    regulator_notes: "Regulator notes",
  };
  const rowsForLot = ((orgSnapshotData && orgSnapshotData.pdc_values) || []).filter(
    (value) => {
      if (!value.payload) return false;
      try {
        return JSON.parse(value.payload).lot === seq;
      } catch {
        return false;
      }
    },
  );
  const disseminated = ((latest && latest.pdc) || [])
    .flatMap((entry) => entry.commitments || []).length;
  if (!rowsForLot.length) {
    pdcBody.append(drawerParagraph(
      view
        ? "No private payload for this lot is readable by you yet — the commitments are what the chain holds."
        : "The chain's commitments for this lot.",
    ));
  }
  for (const value of rowsForLot) {
    const tr = document.createElement("tr");
    let terms = null;
    try {
      terms = JSON.parse(value.payload);
    } catch {}
    const detail = terms ? describeTerms(terms) : "—";
    for (const text of [KIND_LABEL[terms?.kind] ?? "Terms", value.author, detail, `${value.commitment.slice(0, 12)}…`]) {
      const td = document.createElement("td");
      td.append(document.createTextNode(text));
      tr.append(td);
    }
    const tdAccess = document.createElement("td");
    const badge = document.createElement("span");
    badge.className = "badge health health-good";
    badge.append(document.createTextNode("Readable"));
    tdAccess.append(badge);
    tr.append(tdAccess);
    pdcBody.append(tr);
  }
  if (disseminated > rowsForLot.length) {
    content.append(drawerParagraph(
      `${disseminated} payload(s) were disseminated this run; you can read ${rowsForLot.length} of them. The rest stay commitments to you.`,
    ));
  }
  pdcTable.append(pdcBody);
  content.append(pdcTable);
  content.append(drawerParagraph(
    `Custody chain: ${stages.map((stage) => stage.custodian).join(" → ")}`,
  ));
  showDrawer();
}

function openCertDrawer(cert) {
  const content = drawerShell(
    cert.record_id,
    `${cert.schema} · issuer ${cert.issuer}`,
    false,
    true,
  );
  content.append(drawerParagraph(
    `${cert.schema === "quality_certification" ? "Quality certification" : "Audit attestation"} for ${cert.lot_ref} · status ${cert.status}`,
  ));
  // The related chain: the certified lot's custody path from the page
  // snapshot, highlighted on the live graph while this drawer is open.
  const lot = ((latest && latest.lots) || []).find(
    (candidate) =>
      candidate.lot_ref.toLowerCase() === cert.lot_ref.toLowerCase(),
  );
  const h4 = document.createElement("h4");
  h4.append(document.createTextNode("Related chain"));
  content.append(h4);
  if (lot && lot.chain.length) {
    const list = document.createElement("ul");
    for (const stage of lot.chain) {
      const li = document.createElement("li");
      li.append(document.createTextNode(
        `${stage.event_type} → ${stage.custodian} · block ${stage.block}`,
      ));
      list.append(li);
    }
    content.append(list);
    highlightPath = lot.chain.map((stage) => stage.custodian);
    content.append(drawerParagraph(
      `Custody chain: ${lot.chain.map((stage) => stage.custodian).join(" → ")} · status ${lot.status}`,
    ));
  } else {
    content.append(drawerParagraph(
      "The certified lot's custody chain has not committed yet — certifications are issued the round after manufacture, so this should fill shortly.",
    ));
  }
  const h4b = document.createElement("h4");
  h4b.append(document.createTextNode("PDC info"));
  content.append(h4b);
  const seq = Number(cert.lot_ref.replace("LOT-", "").replace("lot-", "")) || 0;
  const view = currentViewOrg();
  const lotPayload = ((orgSnapshotData && orgSnapshotData.pdc_values) || []).find(
    (value) => {
      if (!value.payload) return false;
      try {
        return JSON.parse(value.payload).lot === seq;
      } catch {
        return false;
      }
    },
  );
  if (view === "admin") {
    content.append(drawerParagraph("Admin (demo): the payload's cleartext is readable below."));
    if (lotPayload) {
      content.append(drawerParagraph(lotPayload.summary || "—"));
    } else {
      content.append(drawerParagraph("No private payload was disseminated for this lot yet."));
    }
  } else if (!view) {
    content.append(drawerParagraph(
      "Public view: the chain holds only the sha256 commitment — no cleartext is readable. Use the member selector to inspect as a member.",
    ));
  } else if (lot.manufacturer === view) {
    content.append(drawerParagraph(
      `Your own terms as ${view}: readable below.`,
    ));
    if (lotPayload?.summary) {
      content.append(drawerParagraph(lotPayload.summary));
    } else {
      content.append(drawerParagraph(
        "The private terms for this lot have not reached your node yet — its commitment is on the chain and the cleartext arrives with dissemination.",
      ));
    }
  } else {
    content.append(drawerParagraph(
      `${lot.manufacturer}'s terms — not readable by ${view}. Their cleartext stays with the author and the regulator; this member sees the tamper-evident commitment.`,
    ));
  }
  showDrawer();
}

function openTxDrawer(tx) {
  const content = drawerShell("Transaction", tx.kind, false, true);
  content.append(drawerParagraph(tx.label));
  const rows = [
    ["Transaction id", tx.id],
    ["Kind", tx.kind],
    ["Flow", `${tx.from} → ${tx.to}`],
    ["Committed height", String(tx.height)],
  ];
  const dl = document.createElement("dl");
  dl.className = "metrics";
  for (const [label, value] of rows) {
    const box = document.createElement("div");
    const dt = document.createElement("dt");
    dt.append(document.createTextNode(label));
    const dd = document.createElement("dd");
    dd.append(document.createTextNode(value));
    box.append(dt, dd);
    box.title = label;
    dl.append(box);
  }
  content.append(dl);
  content.append(drawerParagraph(
    "Every dot on the graph is a real transaction: it was submitted on its origin member's node, relayed, admitted, and committed on-chain. Admission is not finality until the block commits.",
  ));
  showDrawer();
}

function openDrawerForEvent(event) {
  const content = drawerShell(event.actor, "adversary outcome", false, false);
  content.append(
    drawerParagraph(`${event.action} — ${event.outcome}`),
    drawerParagraph(String(event.explanation || event.detail || "").replace(/\s+/g, " ").trim()),
  );
  showDrawer();
}

$("drawer-close").addEventListener("click", closeDrawer);
$("drawer-backdrop").addEventListener("click", closeDrawer);

// Drag the bar under a scrollable element to resize its height.
function attachResizer(handle, target) {
  let startY = 0;
  let startH = 0;
  handle.addEventListener("mousedown", (event) => {
    startY = event.clientY;
    startH = target.getBoundingClientRect().height;
    event.preventDefault();
    const move = (moveEvent) => {
      const height = Math.min(
        window.innerHeight * 0.8,
        Math.max(8, startH + (moveEvent.clientY - startY)),
      );
      target.style.maxHeight = `${Math.round(height)}px`;
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
}
attachResizer($("lots-resize"), $("lots").closest(".table-scroll") || $("lots"));
// Every other scroll-wrapped table gets its own drag handle.
document.querySelectorAll(".table-scroll").forEach((container) => {
  if (container.previousElementSibling?.classList?.contains("resize-handle")) return;
  const handle = document.createElement("div");
  handle.className = "resize-handle";
  handle.title = "Drag to resize this table";
  container.after(handle);
  attachResizer(handle, container);
});
attachDrawerResizer();


// ── WebGPU renderer (the step-5 experiment) ─────────────────────────────────

let gpu = null;

const WGSL = `
struct Uniforms { resolution: vec2f };
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VsOut {
  @builtin(position) position: vec4f,
  @location(0) tint: vec3f,
};

@vertex
fn vs(@builtin(vertex_index) corner: u32,
      @location(0) place: vec2f,
      @location(1) radius: f32,
      @location(2) tint: vec3f) -> VsOut {
  let corners = array(
    vec2f(-0.6, -0.6), vec2f(0.6, -0.6), vec2f(-0.6, 0.6),
    vec2f(-0.6, 0.6), vec2f(0.6, -0.6), vec2f(0.6, 0.6),
  );
  let offset = corners[corner] * vec2f(radius);
  let tuned = (place + offset) / uniforms.resolution * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
  var out: VsOut;
  out.position = vec4f(tuned.x, tuned.y, 0.0, 1.0);
  out.tint = tint / vec3f(255.0);
  return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
  return vec4f(in.tint, 1.0);
}
`;

async function tryWebGPU() {
  if (!navigator.gpu) return null;
  try {
    const adapter = await navigator.gpu.requestAdapter();
    if (!adapter) return null;
    const device = await adapter.requestDevice();
    device.lost.then(() => {
      gpu = null;
      setRendererMode("Canvas2D fallback (WebGPU device was lost)");
    });
    const context = canvas.getContext("webgpu");
    const format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({ device, format, alphaMode: "opaque" });
    const module = device.createShaderModule({ code: WGSL });
    const pipeline = device.createRenderPipeline({
      layout: "auto",
      vertex: {
        module,
        entryPoint: "vs",
        buffers: [{
          arrayStride: 24,
          attributes: [
            { shaderLocation: 0, offset: 0, format: "float32x2" },
            { shaderLocation: 1, offset: 8, format: "float32" },
            { shaderLocation: 2, offset: 12, format: "float32x3" },
          ],
        }],
      },
      fragment: { module, entryPoint: "fs", targets: [{ format }] },
      primitive: { topology: "triangle-list" },
    });
    const uniforms = device.createBuffer({
      size: 16,
      usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
    });
    device.queue.writeBuffer(
      uniforms,
      0,
      new Float32Array([canvas.width, canvas.height, 0, 0]),
    );
    // One fixed allocation for the largest load; refunded when the device is
    // lost (the fallback path re-acquires permission and re-runs Canvas2D).
    const instances = device.createBuffer({
      size: (nodePoints().length + TARGET_LOAD) * 24,
      usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
    });
    const bindGroup = device.createBindGroup({
      layout: pipeline.getBindGroupLayout(0),
      entries: [{ binding: 0, resource: { buffer: uniforms } }],
    });
    gpu = { device, context, format, pipeline, instances, bindGroup };
    return gpu;
  } catch (error) {
    console.warn("WebGPU unavailable:", error);
    return null;
  }
}

function drawWebGPU(when) {
  // The experiment draws discs only (no labels/edges — the Canvas2D
  // baseline remains the complete renderer until the gate is measured).
  const radius = nodeRadius();
  const nodes = nodePoints().map((node) => ({
    x: node.x,
    y: node.y,
    color: node.evil ? EVIL_COLOR : ROLE_COLORS[node.role] || [128, 128, 128],
  }));
  const dots = edgePoints(when).map((dot) => ({
    x: dot.x,
    y: dot.y,
    size: 3,
    color: DOT_COLOR,
  }));
  const points = nodes.map((node) => ({ ...node, size: radius })).concat(dots);
  const packed = new Float32Array(points.length * 6);
  points.forEach((point, index) => {
    packed.set(
      [
        point.x * canvas.width,
        point.y * canvas.height,
        point.size,
        ...point.color,
      ],
      index * 6,
    );
  });
  gpu.device.queue.writeBuffer(gpu.instances, 0, packed);
  const encoder = gpu.device.createCommandEncoder();
  const pass = encoder.beginRenderPass({
    colorAttachments: [{
      view: gpu.context.getCurrentTexture().createView(),
      clearValue: { r: 0.05, g: 0.1, b: 0.14, a: 1 },
      loadOp: "clear",
      storeOp: "store",
    }],
  });
  pass.setPipeline(gpu.pipeline);
  pass.setVertexBuffer(0, gpu.instances);
  pass.draw(6, points.length);
  pass.end();
  gpu.device.queue.submit([encoder.finish()]);
}

// ── Renderer decision (adopt-WebGPU-only-on-measured-benefit gate) ──────────

function setRendererMode(label) {
  $("renderer-info").textContent = `renderer: ${label}`;
}

function benchmarkFrameTimes(seconds = 5) {
  const samples = [];
  measuring = true;
  return new Promise((resolve) => {
    const started = performance.now();
    let prior = started;
    function measure(when) {
      samples.push(when - prior);
      prior = when;
      if (when - started < seconds * 1000) {
        requestAnimationFrame(measure);
      } else {
        measuring = false;
        samples.sort((left, right) => left - right);
        resolve(samples[Math.floor(samples.length * 0.95)]);
      }
    }
    requestAnimationFrame(measure);
  });
}

async function decideRenderer(canvasEl) {
  canvas = canvasEl;
  ctx2d = canvas.getContext("2d");
  attachGraphInteractivity(canvasEl);
  requestAnimationFrame(loop);

  const p95 = await benchmarkFrameTimes();
  if (p95 <= FRAME_BUDGET_MS) {
    setRendererMode(`Canvas2D (p95 ${p95.toFixed(1)} ms — within the 10 ms budget)`);
    return;
  }
  const webgpu = await tryWebGPU();
  setRendererMode(
    webgpu
      ? `WebGPU activated (Canvas2D p95 ${p95.toFixed(1)} ms missed the 10 ms budget)`
      : `Canvas2D fallback (10 ms budget missed at ${p95.toFixed(1)} ms; no compatible adapter)`,
  );
}

function loop(when) {
  dotLoad = Math.min(
    TARGET_LOAD,
    6 + ((latest?.metrics?.submitted ?? 0) % 100) * 4,
  );
  if (gpu) {
    drawWebGPU(when);
  } else if (ctx2d) {
    drawCanvas2D(when);
  }
  requestAnimationFrame(loop);
}

// ── Boot ────────────────────────────────────────────────────────────────────

await bootstrap();
await refetchSnapshot();
subscribeEvents();
decideRenderer($("federation"));

export { renderDOM, renderOffers, renderVisibilityMatrixFromRun, renderIfChanged, refetchSnapshot, updateViewBanner, openLotDrawer, openMemberDrawer, openCertDrawer, closeDrawer };
