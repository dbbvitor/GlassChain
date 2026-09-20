// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
// Panel renderers for the GlassChain demo. Pure DOM construction, no
// framework, no innerHTML with data. Every panel is diff-rendered: it only
// rebuilds when the data behind it actually changed, so hover, selection and
// typed input survive the 500 ms snapshot tick.

const SIGNATURES = new Map();

/** Rebuild a panel only when its data signature changed. */
export function renderIfChanged(key, signature, rebuild) {
  if (SIGNATURES.get(key) === signature) return;
  SIGNATURES.set(key, signature);
  rebuild();
}

/** Tiny element helper: el("tr", { class: "clickable", onclick }, child…). */
export function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === null || value === undefined) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key === "dataset") Object.assign(node.dataset, value);
    else if (key.startsWith("on") && typeof value === "function") {
      node.addEventListener(key.slice(2).toLowerCase(), value);
    } else node.setAttribute(key, String(value));
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child.nodeType ? child : document.createTextNode(String(child)));
  }
  return node;
}

export function fmtInt(value) {
  return Number(value || 0).toLocaleString("en-US");
}

/** Minor currency units → "$1,234.56". */
export function fmtMoney(minor) {
  return `$${(Number(minor || 0) / 100).toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}

export function badge(text, tone = "muted") {
  return el("span", { class: "badge", dataset: { tone }, text });
}

export function roleLabel(role) {
  if (!role) return "—";
  return role.charAt(0).toUpperCase() + role.slice(1);
}

function empty(text) {
  return el("p", { class: "empty", text });
}

function kpi(label, value, sub, tone) {
  return el(
    "dl",
    { class: "kpi", dataset: tone ? { tone } : {}, title: label },
    el("dt", { text: label }),
    el("dd", { text: String(value) }),
    sub ? el("span", { class: "sub", text: sub }) : null,
  );
}

function table(headers, rows) {
  return el(
    "div",
    { class: "table-wrap" },
    el(
      "table",
      { class: "data" },
      el(
        "thead",
        {},
        el("tr", {}, ...headers.map((head) => el("th", { text: head }))),
      ),
      el("tbody", {}, ...rows),
    ),
  );
}

function kvRow(value, className) {
  return el("td", { class: className, text: value });
}

function pct(value, total) {
  return total > 0 ? `${Math.round((value / total) * 100)}%` : "0%";
}

// ── Overview ───────────────────────────────────────────────────────────────

export function renderOverview(ctx) {
  const { snapshot } = ctx;
  const metrics = snapshot.metrics || {};

  renderIfChanged(
    "overview.kpis",
    JSON.stringify([
      snapshot.chain_height,
      snapshot.round,
      metrics.tx_per_sec,
      metrics.submitted,
      metrics.rejected,
      metrics.lots,
      metrics.commit_p50_ms,
      metrics.commit_p95_ms,
    ]),
    () => {
      document.getElementById("kpis").replaceChildren(
        kpi("Chain height", fmtInt(snapshot.chain_height), "blocks committed"),
        kpi("Round", fmtInt(snapshot.round), "synchronized scenario rounds"),
        kpi("Throughput", fmtInt(metrics.tx_per_sec), "tx/s since start"),
        kpi("Submitted", fmtInt(metrics.submitted), "accepted into the pool"),
        kpi("Rejected", fmtInt(metrics.rejected), "admission rejections", metrics.rejected > 0 ? "warn" : undefined),
        kpi("Lots", fmtInt(metrics.lots), "synthetic lots produced"),
        kpi("Commit p50", `${fmtInt(metrics.commit_p50_ms)} ms`, "dev-PoW mining time"),
        kpi("Commit p95", `${fmtInt(metrics.commit_p95_ms)} ms`, "outliers included on purpose"),
      );
    },
  );

  const feed = (snapshot.feed || []).slice(-16).reverse();
  renderIfChanged("overview.feed", JSON.stringify(feed), () => {
    const node = document.getElementById("feed");
    if (!feed.length) {
      node.replaceChildren(empty("Nothing committed yet — press Start."));
      return;
    }
    node.replaceChildren(
      ...feed.map((item) =>
        el(
          "li",
          {},
          el("span", { class: "when", text: `h${item.height}` }),
          el("span", { text: item.label }),
        ),
      ),
    );
  });
}

// ── Inventory ──────────────────────────────────────────────────────────────

const DONUT_COLORS = [
  "#6e79e4", "#4cb782", "#e0a458", "#c76fd1", "#5ab0d0",
  "#d3b354", "#9a8fe4", "#7ac0a0", "#c97979", "#a0b4c0",
];

export function renderInventory(ctx) {
  const { snapshot, openMember, selectedMember } = ctx;
  const rows = snapshot.wms || [];
  const inventory = snapshot.inventory || {};
  const summary = snapshot.wms_summary || {};

  renderIfChanged("inventory.kpis", JSON.stringify(summary), () => {
    document.getElementById("wms-kpis").replaceChildren(
      kpi("Members", fmtInt(summary.members), "with warehouse state"),
      kpi("Total stock", fmtInt(summary.total_units), "units on hand"),
      kpi("Inventory value", fmtMoney(summary.total_value_minor), "on-hand × $15.00 demo rate"),
      kpi("Retail sold", fmtInt(summary.total_sold_units), "units sold to end customers"),
      kpi("Low stock", fmtInt(summary.low_stock), "members below one retail drain", summary.low_stock > 0 ? "warn" : "good"),
    );
  });

  const barSignature = JSON.stringify([rows, inventory, selectedMember]);
  renderIfChanged("inventory.bars", barSignature, () => {
    const max = Math.max(1, ...rows.map((row) => row.units_here));
    const sorted = [...rows].sort(
      (left, right) => right.units_here - left.units_here || left.company.localeCompare(right.company),
    );
    const node = document.getElementById("bars");
    if (!sorted.length) {
      node.replaceChildren(empty("No warehouse state yet — start the run."));
      return;
    }
    node.replaceChildren(
      ...sorted.map((row) => {
        const sellable = Number(inventory[row.company] ?? 0);
        const low = sellable < 100;
        return el(
          "div",
          {
            class: `bar-row${row.company === selectedMember ? " selected" : ""}${row.evil ? " evil" : ""}`,
            dataset: { role: row.role, member: row.company },
            title: `${row.company}: ${fmtInt(row.units_here)} on hand · ${fmtInt(sellable)} sellable — click to inspect`,
            onclick: () => openMember(row.company),
          },
          el("span", { class: "bar-name", text: row.company }),
          el("span", { class: "bar-track" }, el("span", { class: "bar-fill" })),
          el("span", {
            class: "bar-units",
            text: `${fmtInt(row.units_here)}${low ? " · low" : ""}`,
          }),
        );
      }),
    );
    const fills = node.querySelectorAll(".bar-fill");
    sorted.forEach((row, index) => {
      if (fills[index]) {
        fills[index].style.width = `${Math.round((row.units_here / max) * 100)}%`;
      }
    });
  });

  renderIfChanged("inventory.donut", JSON.stringify(rows), () => {
    const total = rows.reduce((sum, row) => sum + (row.stock_value_minor || 0), 0);
    const sorted = [...rows].sort(
      (left, right) => (right.stock_value_minor || 0) - (left.stock_value_minor || 0),
    );
    const stops = [];
    const legend = [];
    let cursor = 0;
    let others = 0;
    sorted.forEach((row, index) => {
      const share = total > 0 ? (row.stock_value_minor || 0) / total : 0;
      if (share >= 0.04 || index < 5) {
        const from = (cursor / Math.max(1, total)) * 100;
        cursor += row.stock_value_minor || 0;
        const to = (cursor / Math.max(1, total)) * 100;
        const color = DONUT_COLORS[index % DONUT_COLORS.length];
        stops.push(`${color} ${from}% ${to}%`);
        legend.push({ name: row.company, color, pct: share });
      } else {
        cursor += row.stock_value_minor || 0;
        others += row.stock_value_minor || 0;
      }
    });
    if (others > 0) {
      stops.push(`var(--muted) ${((total - others) / Math.max(1, total)) * 100}% 100%`);
      legend.push({ name: "Others", color: "var(--muted)", pct: others / Math.max(1, total) });
    }
    const donut = document.getElementById("donut");
    donut.style.background = stops.length
      ? `conic-gradient(${stops.join(", ")})`
      : "var(--surface-3)";
    document.getElementById("donut-legend").replaceChildren(
      ...legend.map((item) => {
        const dot = el("i");
        dot.style.background = item.color;
        return el(
          "li",
          {},
          dot,
          el("span", { text: item.name }),
          el("span", { class: "pct", text: `${Math.round(item.pct * 100)}%` }),
        );
      }),
    );
  });

  renderIfChanged("inventory.directory", barSignature, () => {
    document.getElementById("directory-body").replaceChildren(
      ...rows.map((row) => {
        const sellable = Number(inventory[row.company] ?? 0);
        const low = sellable < 100;
        return el(
          "tr",
          {
            class: "clickable",
            dataset: { member: row.company },
            onclick: () => openMember(row.company),
          },
          kvRow(row.company),
          kvRow(roleLabel(row.role)),
          kvRow(fmtInt(row.lots_here), "num"),
          kvRow(fmtInt(row.units_here), "num"),
          kvRow(fmtMoney(row.stock_value_minor), "num"),
          kvRow(fmtInt(sellable), "num"),
          kvRow(fmtInt(row.sold_units), "num"),
          el("td", {}, badge(low ? "Low stock" : "Optimal", low ? "warn" : "good")),
        );
      }),
    );
  });
}

// ── Trade ──────────────────────────────────────────────────────────────────

// Per-offer typed quantities survive re-renders (never snapped back).
const typedQty = new Map();

function isPending(offer) {
  return (
    offer.kind === "offer" &&
    offer.sold < offer.quantity &&
    (offer.note.includes("awaiting") || offer.note.includes("still on offer"))
  );
}

function offerCard(offer, ctx, pending, canBuy) {
  const remaining = offer.quantity - offer.sold;
  const card = el(
    "div",
    { class: "offer" },
    el(
      "div",
      { class: "offer-top" },
      el("span", { class: "offer-product", text: offer.product }),
      el("span", { class: "offer-price", text: fmtMoney(offer.price_per_unit) }),
    ),
    el("div", {
      class: "offer-meta",
      text: offer.kind === "purchase"
        ? `${fmtInt(offer.quantity)} units · ${offer.seller} → ${offer.buyer}`
        : `${fmtInt(offer.sold)} / ${fmtInt(offer.quantity)} units sold · ${offer.seller} → ${offer.buyer}`,
    }),
    el("div", { class: "offer-note", text: offer.note }),
  );
  if (pending && canBuy) {
    const wanted = Math.min(typedQty.get(offer.tx_id) ?? remaining, remaining) || remaining;
    const input = el("input", {
      type: "number",
      min: "1",
      max: String(remaining),
      value: String(wanted),
      title: `Units to buy (1–${remaining} available)`,
      "aria-label": `Units to buy from ${offer.seller}`,
      oninput: () => {
        const value = Number(input.value);
        if (Number.isFinite(value)) typedQty.set(offer.tx_id, value);
      },
    });
    card.append(
      el(
        "div",
        { class: "offer-buy" },
        input,
        el("button", {
          class: "btn btn-primary btn-sm",
          text: "Buy",
          title: "Submit a real PurchaseOrder through the viewing pharmacy's node",
          onclick: () => {
            const typed = Number(input.value) || remaining;
            ctx.buy(offer, Math.min(typed, remaining));
          },
        }),
      ),
    );
  }
  return card;
}

export function renderTrade(ctx) {
  const { snapshot } = ctx;
  const offers = (snapshot.offers || []).filter((event) => event.kind === "offer");
  const purchases = (snapshot.offers || []).filter((event) => event.kind === "purchase");
  const pending = offers.filter(isPending).slice(-6).reverse();
  const filled = offers.filter((offer) => !isPending(offer)).slice(-6).reverse();
  const executed = [...purchases.slice(-8).reverse(), ...filled].slice(0, 10);

  const canBuy = snapshot.status === "running" || snapshot.status === "rebuilding";
  const signature = JSON.stringify([
    offers.map((offer) => [offer.tx_id, offer.sold, offer.note]),
    purchases.map((order) => order.tx_id),
    snapshot.status,
  ]);
  renderIfChanged("trade.offers", signature, () => {
    const pendingNode = document.getElementById("pending-offers");
    pendingNode.replaceChildren(
      ...(pending.length
        ? pending.map((offer) => offerCard(offer, ctx, true, canBuy))
        : [empty("Nothing pending — every advertised offer is filled.")]),
      ...(pending.length && !canBuy
        ? [el("p", { class: "drawer-note", text: "Start the run to complete a pending offer — buys go through a live member's node." })]
        : []),
    );
    const executedNode = document.getElementById("executed-offers");
    executedNode.replaceChildren(
      ...(executed.length
        ? executed.map((entry) => offerCard(entry, ctx, false, false))
        : [empty("No executed purchases yet — start the run.")]),
      el("p", {
        class: "drawer-note",
        text: "Every card is a SupplyOffer or PurchaseOrder the contract engine committed. The engine is real GlassChain code — no staged outcomes.",
      }),
    );
  });

  renderIfChanged("trade.purchases", JSON.stringify(purchases), () => {
    const body = document.getElementById("purchases-body");
    if (!purchases.length) {
      body.replaceChildren(el("tr", {}, el("td", { colSpan: 8 }, empty("No purchases yet."))));
      return;
    }
    body.replaceChildren(
      ...purchases.slice(-12).reverse().map((order) =>
        el(
          "tr",
          {
            class: "clickable",
            title: "Click for the full PurchaseOrder, its contract and its flow",
            onclick: () => ctx.openPurchase(order),
          },
          el("td", { class: "mono", text: `${order.tx_id.slice(0, 10)}…` }),
          kvRow(order.product),
          kvRow(order.seller),
          kvRow(order.buyer),
          kvRow(fmtInt(order.quantity), "num"),
          kvRow(fmtMoney(order.price_per_unit), "num"),
          kvRow(fmtInt(order.round), "num"),
          el("td", {}, badge(order.note.includes("manual") ? "manual" : "auto", order.note.includes("manual") ? "accent" : "good")),
        ),
      ),
    );
  });

  const contracts = snapshot.contracts || [];
  renderIfChanged("trade.contracts", JSON.stringify(contracts), () => {
    const body = document.getElementById("contracts-body");
    if (!contracts.length) {
      body.replaceChildren(el("tr", {}, el("td", { colSpan: 9 }, empty("No contracts registered yet."))));
      return;
    }
    body.replaceChildren(
      ...contracts.map((contract) =>
        el(
          "tr",
          {
            class: "clickable",
            title: `${contract.id} — conditions, activity and its committed purchases`,
            onclick: () => ctx.openContract(contract),
          },
          el("td", { class: "mono", text: contract.id }),
          kvRow(contract.buyer),
          kvRow(contract.product),
          kvRow(fmtMoney(contract.max_price_per_unit), "num"),
          kvRow(fmtInt(contract.max_quantity), "num"),
          el("td", {}, badge(contract.auto_execute ? "auto-execute" : "human approval", contract.auto_execute ? "good" : "accent")),
          kvRow(fmtInt(contract.executions), "num"),
          kvRow(fmtInt(contract.quantity_purchased), "num"),
          el("td", {}, badge(contract.status, "muted")),
        ),
      ),
    );
  });
}

// ── Traceability ───────────────────────────────────────────────────────────

export function renderTraceability(ctx) {
  const { snapshot, openLot, openCert, openBlock } = ctx;
  const lots = snapshot.lots || [];
  const certs = snapshot.certs || [];
  const blocks = snapshot.blocks || [];

  renderIfChanged("traceability.kpis", JSON.stringify([lots.length, certs.length, blocks.length, snapshot.chain_height]), () => {
    const complete = lots.filter((lot) => lot.status === "complete").length;
    const events = lots.reduce((sum, lot) => sum + (lot.chain || []).length, 0);
    document.getElementById("trace-kpis").replaceChildren(
      kpi("Lots tracked", fmtInt(lots.length), "from anchor to pharmacy"),
      kpi("Complete", fmtInt(complete), "full custody chain committed"),
      kpi("Custody events", fmtInt(events), "committed provenance hops"),
      kpi("Attestations", fmtInt(certs.length), "certifications + audits"),
      kpi("Blocks", fmtInt(blocks.length), `of height ${fmtInt(snapshot.chain_height)}`),
    );
  });

  renderIfChanged(
    "traceability.lots",
    JSON.stringify(lots.map((lot) => [lot.lot_ref, lot.status, lot.manufacturer, lot.trust_score, lot.lineage_complete, lot.chain])),
    () => {
      document.getElementById("lots-body").replaceChildren(
        ...lots.slice().reverse().map((lot) => {
          const chain = (lot.chain || [])
            .map((step) => `${step.event_type} → ${step.custodian} @${step.block}`)
            .join("  ·  ");
          const score = lot.trust_score ?? 0;
          return el(
            "tr",
            {
              class: "clickable",
              title: `${lot.lot_ref} — full lineage, PDC info and graph path`,
              onclick: () => openLot(lot),
            },
            kvRow(lot.lot_ref),
            el("td", {}, badge(lot.status, lot.status === "complete" ? "accent" : "muted")),
            el("td", {}, badge(lot.lineage_complete ? "lineage verified" : "lineage pending", lot.lineage_complete ? "good" : "muted")),
            kvRow(lot.manufacturer || "—"),
            kvRow(`${score}/100`, "num"),
            kvRow(chain || "not committed yet", "clip"),
          );
        }),
      );
    },
  );

  renderIfChanged("traceability.certs", JSON.stringify(certs), () => {
    const body = document.getElementById("certs-body");
    if (!certs.length) {
      body.replaceChildren(el("tr", {}, el("td", { colSpan: 5 }, empty("No certifications yet — they follow a lot's first round."))));
      return;
    }
    body.replaceChildren(
      ...certs.slice().reverse().map((cert) =>
        el(
          "tr",
          {
            class: "clickable",
            title: `${cert.record_id} — details and the related custody chain`,
            onclick: () => openCert(cert),
          },
          el("td", { class: "mono", text: cert.record_id }),
          kvRow(cert.schema === "quality_certification" ? "Quality certification" : "Audit attestation"),
          kvRow(cert.lot_ref),
          kvRow(cert.issuer),
          el("td", {}, badge(cert.status, "good")),
        ),
      ),
    );
  });

  renderIfChanged("traceability.blocks", JSON.stringify(blocks), () => {
    const body = document.getElementById("blocks-body");
    if (!blocks.length) {
      body.replaceChildren(el("tr", {}, el("td", { colSpan: 6 }, empty("No blocks committed yet."))));
      return;
    }
    body.replaceChildren(
      ...blocks.map((block) =>
        el(
          "tr",
          { class: "clickable", title: "Each block hash commits to the previous one", onclick: () => openBlock(block) },
          kvRow(fmtInt(block.height), "num"),
          el("td", { class: "mono", text: block.hash }),
          el("td", { class: "mono", text: block.previous_hash }),
          kvRow(fmtInt(block.tx_count), "num"),
          kvRow(new Date(block.timestamp * 1000).toLocaleTimeString(), ""),
          el("td", {}, badge(block.certified ? "QC" : "PoW", block.certified ? "good" : "muted")),
        ),
      ),
    );
  });
}

// ── Trust & Security ───────────────────────────────────────────────────────

export function renderSecurity(ctx) {
  const { snapshot, openSecurity, openMember, view, orgView } = ctx;
  const events = snapshot.security || [];
  const posture = snapshot.posture || [];

  renderIfChanged("security.kpis", JSON.stringify([events.length, posture]), () => {
    const verified = posture.filter((row) => row.ocsp.includes("verified locally")).length;
    const verifiers = posture.filter((row) => row.verifier).length;
    document.getElementById("security-kpis").replaceChildren(
      kpi("Members", fmtInt(posture.length), "identity issued by one demo Root CA"),
      kpi("Verifier on", fmtInt(verifiers), "org paths fail closed without one"),
      kpi("OCSP verified", fmtInt(verified), "issuer-signed staples, checked locally"),
      kpi("Attacks logged", fmtInt(events.length), "real gates, real outcomes"),
      kpi("Zero-trust rejections", fmtInt(events.filter((event) => event.outcome.includes("rejected") || event.outcome.includes("fail-closed")).length), "nothing staged"),
    );
  });

  renderIfChanged(
    "security.posture",
    JSON.stringify([posture, (snapshot.orgs || []).map((org) => [org.id, org.trust_score, org.records])]),
    () => {
      const body = document.getElementById("posture-body");
      if (!posture.length) {
        body.replaceChildren(el("tr", {}, el("td", { colSpan: 8 }, empty("Posture appears once the run is started."))));
        return;
      }
      const orgs = snapshot.orgs || [];
      body.replaceChildren(
        ...posture.map((row) => {
          const org = orgs.find((candidate) => candidate.id === row.company);
          const trust =
            org && org.records > 0
              ? badge(`${org.trust_score}/100 · ${fmtInt(org.records)} rec`, org.trust_score >= 80 ? "good" : "warn")
              : badge("no records", "muted");
          return el(
            "tr",
            {
              class: `clickable${row.evil ? " evil" : ""}`,
              dataset: { member: row.company },
              title: `${row.company} — verifier, certificate, staple, channels; click to inspect the member`,
              onclick: () => openMember(row.company),
            },
            kvRow(row.company),
            kvRow(roleLabel(row.role)),
            el("td", { title: "Average MetadataTrustScore over registrations this org originated" }, trust),
            el("td", {}, badge(row.verifier ? "enforcing" : "no verifier", row.verifier ? "good" : "danger")),
            el("td", {}, badge(row.certificate ? "X.509 issued" : "none", row.certificate ? "good" : "danger")),
            el("td", {}, badge(row.ocsp, row.ocsp.includes("verified locally") ? "good" : "warn")),
            kvRow((row.collections || []).join(", ") || "—"),
            kvRow(fmtInt(row.peers), "num"),
          );
        }),
      );
    },
  );

  renderIfChanged(
    "security.events",
    JSON.stringify(events.map((event) => [event.actor, event.action, event.outcome, event.detail])),
    () => {
      document.getElementById("security-body").replaceChildren(
        ...events.slice().reverse().map((event) =>
          el(
            "tr",
            {
              class: "clickable evil",
              title: "Click for the gate that answered and why",
              onclick: () => openSecurity(event),
            },
            kvRow(event.actor),
            kvRow(event.action),
            el("td", {}, badge(event.outcome, event.outcome.includes("admitted") ? "warn" : "good")),
            kvRow(event.detail),
          ),
        ),
      );
    },
  );

  renderIfChanged(
    "security.visibility",
    JSON.stringify([snapshot.orgs, snapshot.chain_height, view, orgView ? orgView.pdc_values?.length : null]),
    () => {
      document.getElementById("visibility-body").replaceChildren(
        ...(snapshot.orgs || []).map((org) => {
          const own = view === org.id && orgView && orgView.org === view ? (orgView.pdc_values || []) : null;
          let payloads;
          let tone;
          if (org.evil) {
            payloads = "not a member";
            tone = "danger";
          } else if (own) {
            payloads = `${own.length} held privately`;
            tone = "accent";
          } else if (org.member_of.length) {
            payloads = "holds payloads";
            tone = "good";
          } else {
            payloads = "commitments only";
            tone = "muted";
          }
          return el(
            "tr",
            {
              class: `clickable${org.evil ? " evil" : ""}`,
              dataset: { member: org.id },
              title: `${org.id} — click to inspect what this member can see`,
              onclick: () => openMember(org.id),
            },
            kvRow(org.id),
            kvRow(roleLabel(org.role)),
            el(
              "td",
              {},
              org.records > 0
                ? badge(`${org.trust_score}/100`, org.trust_score >= 80 ? "good" : "warn")
                : badge("—", "muted"),
            ),
            kvRow(`height ${fmtInt(snapshot.chain_height)}`, "num"),
            el("td", {}, badge(payloads, tone)),
          );
        }),
      );
    },
  );

  const proofs = snapshot.equivocations || [];
  renderIfChanged("security.equivocations", JSON.stringify(proofs), () => {
    const card = document.getElementById("equivocation-card");
    card.hidden = proofs.length === 0;
    document.getElementById("equivocations").replaceChildren(
      ...proofs.map((proof) => el("li", { text: proof })),
    );
  });
}

// ── Compliance ─────────────────────────────────────────────────────────────

export function renderCompliance(ctx) {
  const { snapshot, openLot } = ctx;
  const compliance = snapshot.compliance || {};
  const lots = snapshot.lots || [];
  const records = compliance.recent || [];

  renderIfChanged("compliance.kpis", JSON.stringify(compliance), () => {
    const checked = (compliance.compliant || 0) + (compliance.non_compliant || 0);
    document.getElementById("compliance-kpis").replaceChildren(
      kpi("Lots compliant", `${fmtInt(compliance.compliant)}/${fmtInt(checked)}`, `SNCM schema ${compliance.schema_version || "v1"}`, compliance.non_compliant > 0 ? "warn" : "good"),
      kpi("Schema coverage", `${compliance.fields_present || 0}/${compliance.fields_total || 6}`, "mandatory + recommended fields"),
      kpi("Critical findings", fmtInt(compliance.critical), "would block a compliant submission", compliance.critical > 0 ? "danger" : "good"),
      kpi("Flat records", fmtInt(compliance.flat_records), "analytical projection of the chain"),
      kpi("Standard compliant", fmtInt(compliance.standard_records), "trust score ≥ 80"),
      kpi("Flagged records", fmtInt(compliance.low_trust_records), "missing core fields, visible on-chain", compliance.low_trust_records > 0 ? "warn" : "good"),
      kpi("Lineage verified", `${fmtInt(compliance.lineages_complete)}/${fmtInt(compliance.lineages_checked)}`, "custody events ↔ analytical records"),
      kpi("Avg trust", `${fmtInt(compliance.avg_trust)}/100`, "fleet-wide"),
    );
  });

  renderIfChanged(
    "compliance.schema",
    JSON.stringify(lots.map((lot) => [lot.lot_ref, lot.manufacturer, lot.schema_compliant, lot.lineage_complete, lot.trust_score, lot.trust_avg])),
    () => {
      document.getElementById("schema-body").replaceChildren(
        ...lots.slice().reverse().map((lot) =>
          el(
            "tr",
            {
              class: "clickable",
              title: `${lot.lot_ref} — schema, lineage and trust; click for the full lot`,
              onclick: () => openLot(lot),
            },
            kvRow(lot.lot_ref),
            kvRow(lot.manufacturer),
            el("td", {}, badge(lot.schema_compliant ? "compliant" : "non-compliant", lot.schema_compliant ? "good" : "warn")),
            kvRow(lot.schema_compliant ? "0.7× gas" : "1.0× gas"),
            el("td", {}, badge(lot.lineage_complete ? "complete" : "pending", lot.lineage_complete ? "good" : "muted")),
            kvRow(`${lot.trust_score} → ${lot.trust_avg}`, "num"),
          ),
        ),
      );
    },
  );

  renderIfChanged("compliance.trust", JSON.stringify([compliance.standard_records, compliance.low_trust_records, compliance.avg_trust]), () => {
    const total = Math.max(1, (compliance.standard_records || 0) + (compliance.low_trust_records || 0));
    const rows = [
      { name: "Standard compliant", value: compliance.standard_records || 0, color: "var(--good)" },
      { name: "Flagged (low trust)", value: compliance.low_trust_records || 0, color: "var(--warn)" },
      { name: "Average trust", value: compliance.avg_trust || 0, color: "var(--accent)", scale: 100 },
    ];
    document.getElementById("trust-chart").replaceChildren(
      ...rows.map((row) => {
        const fill = el("span", { class: "bar-fill" });
        fill.style.width = `${Math.round((row.value / (row.scale ? 100 : total)) * 100)}%`;
        if (row.color) fill.style.background = row.color;
        return el(
          "div",
          { class: "bar-row", title: `${row.name}: ${row.value}` },
          el("span", { class: "bar-name", text: row.name }),
          el("span", { class: "bar-track" }, fill),
          el("span", { class: "bar-units", text: fmtInt(row.value) }),
        );
      }),
    );
  });

  renderIfChanged("compliance.records", JSON.stringify(records), () => {
    const body = document.getElementById("flat-body");
    if (!records.length) {
      body.replaceChildren(el("tr", {}, el("td", { colSpan: 8 }, empty("No analytical records yet."))));
      return;
    }
    const lotByBatch = new Map();
    for (const lot of lots) {
      const seq = Number(String(lot.lot_ref).replace(/\D/g, ""));
      lotByBatch.set(`B-${String(seq).padStart(4, "0")}`, lot);
    }
    body.replaceChildren(
      ...records.map((record) => {
        const lot = lotByBatch.get(record.batch);
        return el(
          "tr",
          lot
            ? {
                class: "clickable",
                title: `${record.batch} belongs to ${lot.lot_ref} — click for the full lot`,
                onclick: () => openLot(lot),
              }
            : {},
          kvRow(fmtInt(record.block), "num"),
          kvRow(record.gtin || "—", "mono"),
          kvRow(record.batch || "—"),
          kvRow(record.serial || "missing", record.serial ? "" : "bad-cell"),
          kvRow(record.custodian),
          kvRow(record.event),
          el("td", {}, badge(`${record.trust}/100`, record.standard ? "good" : "warn")),
          kvRow(record.missing || "—"),
        );
      }),
    );
  });
}

// ── Performance ────────────────────────────────────────────────────────────

function chart(container, points, key, tone, format) {
  const values = points.map((point) => Number(point[key] || 0));
  const max = Math.max(1, ...values);
  container.replaceChildren(
    el(
      "div",
      { class: "chart" },
      ...points.map((point, index) => {
        const bar = el("span", {
          class: "chart-bar",
          title: `round ${point.round}: ${format(values[index])}`,
        });
        bar.style.height = `${Math.max(3, Math.round((values[index] / max) * 100))}%`;
        bar.dataset.tone = tone;
        return bar;
      }),
    ),
    el(
      "div",
      { class: "chart-axis" },
      el("span", { text: points.length ? `round ${points[0].round} → ${points.at(-1).round}` : "no rounds yet" }),
      el("span", { text: `max ${format(max)}` }),
    ),
  );
}

function percentileOf(values, pct) {
  if (!values.length) return 0;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * (pct / 100)))];
}

// Where a round's wall-clock goes, in draw order (bottom → top of each bar).
const ROUND_PHASES = [
  ["scenario", "produce_ms", "#6e79e4"],
  ["payloads", "payload_ms", "#9a8fe4"],
  ["submit", "submit_ms", "#4ea7fc"],
  ["settle", "settle_ms", "#5ab0d0"],
  ["mine (PoW)", "commit_ms", "#e5a54b"],
  ["project", "project_ms", "#4cb782"],
  ["retail", "retail_ms", "#8b919b"],
];

function breakdownChart(container, history) {
  const roundMs = history.map((point) => point.round_ms || 0);
  const max = Math.max(1, ...roundMs);
  container.replaceChildren(
    el(
      "div",
      { class: "stack-chart" },
      ...history.map((point) => {
        const stack = el("div", {
          class: "stack-col",
          title: `round ${point.round}: ${point.round_ms} ms total · ` +
            ROUND_PHASES.map(([label, key]) => `${label} ${point[key] || 0}`).join(" · "),
        });
        for (const [label, key, color] of ROUND_PHASES) {
          const value = Number(point[key] || 0);
          if (value === 0) continue;
          const seg = el("span", { class: "stack-seg", title: `${label}: ${value} ms` });
          seg.style.height = `${Math.max(1, (value / max) * 100)}%`;
          seg.style.background = color;
          stack.append(seg);
        }
        return stack;
      }),
    ),
    el(
      "div",
      { class: "legend" },
      ...ROUND_PHASES.map(([label, , color]) => {
        const dot = el("i");
        dot.style.background = color;
        return el("span", { class: "legend-item" }, dot, label);
      }),
      el("span", {
        class: "legend-note",
        text: history.length ? `max ${max} ms/round` : "no rounds yet",
      }),
    ),
  );
}

export function renderPerformance(ctx) {
  const { snapshot } = ctx;
  const metrics = snapshot.metrics || {};
  const history = snapshot.history || [];
  const roundP50 = percentileOf(history.map((point) => point.round_ms || 0), 50);
  const roundP95 = percentileOf(history.map((point) => point.round_ms || 0), 95);
  const roundP99 = percentileOf(history.map((point) => point.round_ms || 0), 99);
  const commitments = (snapshot.pdc || []).reduce(
    (sum, entry) => sum + (entry.commitments || []).length,
    0,
  );

  renderIfChanged(
    "performance.grid",
    JSON.stringify([metrics, snapshot.chain_height, commitments, roundP50, roundP95, roundP99]),
    () => {
      document.getElementById("metric-grid").replaceChildren(
        kpi("Submitted", fmtInt(metrics.submitted), "admission, not finality"),
        kpi("Rejected", fmtInt(metrics.rejected), "evil attempts and invalid traffic", metrics.rejected > 0 ? "warn" : undefined),
        kpi("Blocks mined", fmtInt(metrics.blocks), "dev-PoW, difficulty 1"),
        kpi("Lots", fmtInt(metrics.lots), "since the run started"),
        kpi("Chain height", fmtInt(snapshot.chain_height), "including genesis and setup"),
        kpi("Throughput", fmtInt(metrics.tx_per_sec), "tx/s"),
        kpi("Round p50", `${fmtInt(roundP50)} ms`, "measured wall-clock per round"),
        kpi("Round p95", `${fmtInt(roundP95)} ms`, "outliers included"),
        kpi("Round p99", `${fmtInt(roundP99)} ms`, "the tail a stress run must hold"),
        kpi("Elapsed", `${fmtInt(metrics.elapsed_s)} s`, "run wall-clock"),
        kpi("Last commit", `${fmtInt(metrics.last_commit_ms)} ms`, "single block mining time"),
        kpi("Commit p50", `${fmtInt(metrics.commit_p50_ms)} ms`, "rolling 64-block window"),
        kpi("Commit p95", `${fmtInt(metrics.commit_p95_ms)} ms`, "outliers included"),
        kpi("Commit p99", `${fmtInt(metrics.commit_p99_ms)} ms`, "tail commit latency"),
        kpi("Pending pool", fmtInt(metrics.pool_count), "transactions waiting"),
        kpi("Pool bytes", fmtInt(metrics.pool_bytes), "serialized size"),
        kpi("PDC commitments", fmtInt(commitments), "sha256 anchors on-chain"),
      );
    },
  );

  renderIfChanged("performance.throughput", JSON.stringify(history.map((point) => [point.round, point.tx_per_sec])), () => {
    chart(document.getElementById("throughput-chart"), history, "tx_per_sec", "accent", (value) => `${value} tx/s`);
  });
  renderIfChanged("performance.latency", JSON.stringify(history.map((point) => [point.round, point.commit_ms])), () => {
    chart(document.getElementById("latency-chart"), history, "commit_ms", "warn", (value) => `${value} ms`);
  });
  renderIfChanged(
    "performance.breakdown",
    JSON.stringify(history.map((point) => [point.round, point.round_ms, point.produce_ms, point.payload_ms, point.submit_ms, point.settle_ms, point.commit_ms, point.project_ms, point.retail_ms])),
    () => {
      breakdownChart(document.getElementById("round-breakdown"), history);
    },
  );
  renderIfChanged(
    "performance.phases",
    JSON.stringify(history.map((point) => [point.round, point.produce_ms, point.payload_ms, point.submit_ms, point.settle_ms, point.commit_ms, point.project_ms, point.retail_ms, point.round_ms])),
    () => {
      const rows = [
        ...ROUND_PHASES.map(([label, key]) => [label, key]),
        ["round total", "round_ms"],
      ];
      document.getElementById("phase-body").replaceChildren(
        ...rows.map(([label, key]) => {
          const values = history.map((point) => Number(point[key] || 0));
          return el(
            "tr",
            {},
            el("td", { text: label }),
            el("td", { class: "num", text: `${fmtInt(percentileOf(values, 50))} ms` }),
            el("td", { class: "num", text: `${fmtInt(percentileOf(values, 95))} ms` }),
            el("td", { class: "num", text: `${fmtInt(percentileOf(values, 99))} ms` }),
          );
        }),
      );
    },
  );

  const params = snapshot.params || {};
  const info = [
    ["Consensus", "PoW dev/test, difficulty 1"],
    ["Round", fmtInt(snapshot.round)],
    ["Members", fmtInt((snapshot.orgs || []).length)],
    [
      "Topology",
      `${params.manufacturers ?? 0} mfg · ${params.distributors ?? 0} dist · ${params.logistics ?? 0} log · ${params.pharmacies ?? 0} pharm · ${params.regulators ?? 0} reg · ${params.certifiers ?? 0} cert · ${params.evil_nodes ?? 0} evil`,
    ],
    ["Lots / round", fmtInt(params.lots_per_round)],
    ["Round interval", `${fmtInt(params.round_interval_ms)} ms`],
  ];
  renderIfChanged("performance.run", JSON.stringify(info), () => {
    document.getElementById("run-info").replaceChildren(
      ...info.map(([label, value]) =>
        el("div", { class: "pair" }, el("dt", { text: label }), el("dd", { text: value })),
      ),
    );
  });
}

// ── Drawer building blocks (shared with app.js) ────────────────────────────

export { empty, kpi };

export function timeline(stages) {
  return el(
    "ul",
    { class: "timeline" },
    ...stages.map((stage) =>
      el(
        "li",
        {},
        el("div", { class: "event", text: stage.event_type }),
        el("div", { class: "where" }, "custodian ", el("b", { text: stage.custodian }), ` · block ${stage.block}`),
      ),
    ),
  );
}

export function pdcTable(payloads, viewer) {
  if (!payloads.length) {
    return empty(
      viewer
        ? "No private payload authored by this member is readable in this view."
        : "The public lens holds commitments only — choose a member to inspect private terms.",
    );
  }
  return table(
    ["Kind", "Lot", "Terms", "Commitment", "Access"],
    payloads.map((value) =>
      el(
        "tr",
        {},
        el("td", {}, badge(value.kind || "terms", "accent")),
        kvRow(value.lot ? String(value.lot) : "—", "num"),
        kvRow(value.summary || "—"),
        el("td", { class: "mono", text: `${(value.commitment || "").slice(0, 12)}…` }),
        el("td", {}, badge(value.payload ? "readable" : "commitment", value.payload ? "good" : "muted")),
      ),
    ),
  );
}
