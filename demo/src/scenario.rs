// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! The synthetic demo run: a real in-process GlassChain federation whose
//! topology comes from operator-editable parameters — multiple companies per
//! role, evil nodes attacking the real fail-closed paths, contract-driven
//! offer matching, and per-member visibility through their own nodes.
//! Presentation only — never production evidence.

use glasschain_core::{
    capability_hash, validate_asset, CanonicalRecord, CapabilityActivation, InventoryUpdate,
    MetadataTrustScore, PurchaseConditions, PurchaseOrder, RecordSignature, SmartContractDef,
    SupplyOffer, TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
    SCHEMA_VERSION_V1,
};
use glasschain_identity::{CertChainVerifier, Channel, ChannelConfig, OcspStatus, Organization};
use glasschain_network::{Node, NodeEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// Consensus mode label shown verbatim by the UI (dev PoW, not evidence).
pub const MODE_LABEL: &str =
    "PoW dev/test (difficulty 1) — synthetic demonstration, not production evidence";

const COLLECTION: &str = "pricing";
const GTIN: &str = "07891234100016";
const CONTRACT_ID: &str = "auto-replenish";
/// Conditions-only contracts (auto_execute=false): offers priced above the
/// auto cap match exactly one of them by price band and wait for the human
/// buyer named by the contract. Each pharmacy owns one.
const MANUAL_CONTRACT_ID: &str = "manual-review";
const VALUE_CONTRACT_ID: &str = "value-review";
const PREMIUM_CONTRACT_ID: &str = "premium-review";
/// Demo starting cash per company (minor units: $100 000). Presentation
/// bookkeeping — the chain itself carries no currency balance.
const CASH_START: u64 = 10_000_000;
/// Manual price bands (minor units): the band picks the contract and buyer,
/// so a premium offer only ever matches one conditions-only contract.
const MANUAL_MAX_PRICE: u64 = 2_000;
const VALUE_MAX_PRICE: u64 = 1_600;
const PREMIUM_MAX_PRICE: u64 = 2_400;
/// The one auto-executing contract's cap: this is the only band that can
/// produce an automatic `PurchaseOrder`, so caps must not overlap autos.
const CONTRACT_MAX_PRICE: u64 = 1_200;
/// Offer quantity tiers: cheap (auto) offers cover a varied slice of the
/// 500-unit lot; premium (manual) offers are larger so a human has something
/// worth reviewing.
const AUTO_QUANTITY_TIERS: [u64; 10] = [100, 125, 150, 200, 250, 300, 350, 400, 450, 500];
const MANUAL_QUANTITY_TIERS: [u64; 5] = [250, 300, 350, 400, 500];
const AUTO_PRICE_TIERS: [u64; 8] = [850, 900, 950, 1_000, 1_050, 1_100, 1_150, 1_200];
const MANUAL_PRICE_TIERS: [u64; 6] = [1_350, 1_500, 1_700, 1_900, 2_100, 2_300];
/// Retail drain: units a stocked pharmacy sells to customers per round
/// (deliberately slow — inventories accumulate and only sustained rounds
/// empty them). Scales with the pharmacy's own stock so the system can
/// actually reach a steady state instead of piling up forever.
const RETAIL_UNITS_PER_ROUND: u64 = 100;
/// Buy pressure: every 5 000 units of absolute system stock add one step to
/// the purchase multiplier (capped) — full-lot buys when stock piles up,
/// the partial-lot ladder when it is scarce.
const STOCK_PRESSURE_DIVISOR: u64 = 5_000;
const PRESSURE_CAP: u64 = 2;
/// Retail price per unit in minor units ($15.00).
const RETAIL_PRICE: u64 = 1_500;
const FEED_CAP: usize = 60;
const SECURITY_CAP: usize = 30;
const OFFER_CAP: usize = 80;
const COMMIT_WINDOW: usize = 64;

// ── Simulation parameters (operator-editable, even mid run) ─────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct SimParams {
    pub manufacturers: u8,
    pub distributors: u8,
    pub logistics: u8,
    pub pharmacies: u8,
    pub regulators: u8,
    pub certifiers: u8,
    /// Evil companies: alternating flavors — verified-but-nonmember
    /// manufacturers (even) and verifier-less certifiers (odd).
    pub evil_nodes: u8,
    pub lots_per_round: u8,
    pub round_interval_ms: u64,
}

impl Default for SimParams {
    fn default() -> Self {
        Self::defaults()
    }
}

impl SimParams {
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            manufacturers: 3,
            distributors: 2,
            logistics: 2,
            pharmacies: 3,
            regulators: 1,
            certifiers: 2,
            evil_nodes: 2,
            lots_per_round: 3,
            round_interval_ms: 400,
        }
    }

    /// Clamp to the loopback-sane bounds. Values outside these are adjusted,
    /// never rejected, so the demo keeps running.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.manufacturers = self.manufacturers.clamp(1, 6);
        self.distributors = self.distributors.clamp(1, 5);
        self.logistics = self.logistics.clamp(1, 5);
        self.pharmacies = self.pharmacies.clamp(1, 6);
        self.regulators = self.regulators.clamp(0, 3);
        self.certifiers = self.certifiers.clamp(1, 4);
        self.evil_nodes = self.evil_nodes.clamp(0, 5);
        self.lots_per_round = self.lots_per_round.clamp(1, 50);
        self.round_interval_ms = self.round_interval_ms.clamp(0, 10_000);
        self
    }
}

#[derive(Serialize, Clone)]
pub struct Company {
    pub id: String,
    pub role: String,
    pub evil: bool,
    pub has_verifier: bool,
}

/// `(id, role, evil, has_verifier)` per parameter set. Evil distribution:
/// even-indexed evils are manufacturers (verified but excluded from
/// `pricing`), odd-indexed evils are certifiers (no verifier at all).
pub fn build_companies(params: &SimParams) -> Vec<Company> {
    let mut companies = Vec::new();
    let push_role = |count: u8, role: &str, companies: &mut Vec<Company>| {
        for index in 1..=count {
            companies.push(Company {
                id: format!("{role}-{index}"),
                role: role.to_owned(),
                evil: false,
                has_verifier: true,
            });
        }
    };
    push_role(params.manufacturers, "manufacturer", &mut companies);
    push_role(params.distributors, "distributor", &mut companies);
    push_role(params.logistics, "logistics", &mut companies);
    push_role(params.pharmacies, "pharmacy", &mut companies);
    push_role(params.regulators, "regulator", &mut companies);
    push_role(params.certifiers, "certifier", &mut companies);
    for evil_index in 1..=params.evil_nodes {
        let even = evil_index % 2 == 0;
        // Even index → verifier-less certifier (fail-closed gate #86 demo);
        // odd index → verified manufacturer excluded from `pricing`
        // (membership-gate demo).
        companies.push(Company {
            id: format!("evil-{evil_index}"),
            role: if even { "certifier" } else { "manufacturer" }.to_owned(),
            evil: true,
            has_verifier: !even,
        });
    }
    companies
}

fn companies_of_role<'a>(companies: &'a [Company], role: &str) -> Vec<&'a str> {
    companies
        .iter()
        .filter(|company| company.role == role && !company.evil)
        .map(|company| company.id.as_str())
        .collect()
}

// ── Snapshot structs (the JSON the bridge serves) ──────────────────────────

#[derive(Serialize, Clone, Debug)]
pub struct OrgView {
    pub id: String,
    pub role: String,
    pub evil: bool,
    /// Collections this org is a member of; empty for certifiers and evils.
    pub member_of: Vec<String>,
    /// Average MetadataTrustScore over the asset registrations this org
    /// originated (0 with no records — trust is then simply undefined here,
    /// not assumed good).
    pub trust_score: u8,
    /// Committed asset registrations this org originated.
    pub records: u64,
}

#[derive(Serialize, Clone)]
pub struct LotStage {
    pub event_type: String,
    pub custodian: String,
    pub block: u64,
}

#[derive(Serialize, Clone)]
pub struct LotView {
    pub lot_ref: String,
    pub status: String,
    pub manufacturer: String,
    /// Trust score of the originating registration (evil metadata scores low).
    pub trust_score: u8,
    pub chain: Vec<LotStage>,
    /// Every committed custody event has a matching flat analytical record
    /// for the lot's batch (`ProvenanceIndex::verify_lineage`).
    pub lineage_complete: bool,
    /// Average trust score across the lot's flat analytical records.
    pub trust_avg: u8,
    /// Flat analytical records committed for this lot's batch.
    pub flat_records: u64,
    /// The lot's registration passes strict SNCM schema validation.
    pub schema_compliant: bool,
}

#[derive(Serialize, Clone)]
pub struct CertView {
    pub record_id: String,
    pub schema: String,
    pub lot_ref: String,
    pub issuer: String,
    pub status: String,
}

#[derive(Serialize, Clone)]
pub struct SecurityEvent {
    pub actor: String,
    pub action: String,
    pub outcome: String,
    pub detail: String,
    /// The human explanation of which gate answered and why — shown when
    /// the demo's security row is expanded.
    pub explanation: String,
}

/// One recent transaction on the graph: which edge it flows along and what
/// it is, so a clicked dot can explain itself.
#[derive(Serialize, Clone)]
pub struct TxView {
    pub id: String,
    pub kind: String,
    pub from: String,
    pub to: String,
    pub label: String,
    pub height: u64,
}

/// WMS-like warehouse rows, derived from committed custody chains: where
/// lots physically sit right now, and the movement totals per company.
#[derive(Serialize, Clone, Default)]
pub struct WmsRow {
    pub company: String,
    pub role: String,
    pub product: String,
    /// Lots whose latest committed custodian is this company.
    pub lots_here: u64,
    pub units_here: u64,
    /// Cumulative inbound / outbound units from committed events.
    pub received_units: u64,
    pub dispatched_units: u64,
    /// Units sold on to end customers (retail), registered on-chain.
    pub sold_units: u64,
    /// On-hand × retail price (demo bookkeeping — the chain carries no
    /// valuation).
    pub stock_value_minor: u64,
    /// Sellable inventory (the tracked trading pool).
    pub sellable_units: u64,
}

/// Fleet-level WMS KPIs (the header row above the cards).
#[derive(Serialize, Clone, Default)]
pub struct WmsSummary {
    pub members: u64,
    pub total_units: u64,
    pub total_value_minor: u64,
    pub total_sold_units: u64,
    pub low_stock: u64,
}

/// One row of the contract flow: a seller's `SupplyOffer`, the engine's
/// generated `PurchaseOrder` (the match), or a contract execution.
#[derive(Serialize, Clone)]
pub struct OfferEvent {
    pub kind: String,
    pub tx_id: String,
    pub seller: String,
    pub buyer: String,
    pub product: String,
    pub quantity: u64,
    /// Units already bought off this offer (partial manual buys).
    pub sold: u64,
    pub price_per_unit: u64,
    /// Round the offer was advertised in (0 for setup-time events).
    pub round: u64,
    pub note: String,
}

#[derive(Serialize, Clone)]
pub struct EdgeCount {
    pub from: String,
    pub to: String,
    pub count: u64,
}

#[derive(Serialize, Clone)]
pub struct PdcEntry {
    pub collection: String,
    pub commitments: Vec<String>,
}

#[derive(Serialize, Clone, Default)]
pub struct Metrics {
    pub submitted: u64,
    pub rejected: u64,
    pub blocks: u64,
    pub lots: u64,
    pub elapsed_s: u64,
    pub tx_per_sec: u64,
    pub last_commit_ms: u64,
    pub commit_p50_ms: u64,
    pub commit_p95_ms: u64,
    pub commit_p99_ms: u64,
    pub pool_count: u64,
    pub pool_bytes: u64,
}

#[derive(Serialize, Clone)]
pub struct FeedItem {
    pub label: String,
    pub height: u64,
}

/// One committed block, in the tamper-evident chain view: the hash links to
/// the previous block, so changing any committed transaction breaks the chain.
#[derive(Serialize, Clone, Default)]
pub struct BlockView {
    pub height: u64,
    pub hash: String,
    pub previous_hash: String,
    pub tx_count: u64,
    pub timestamp: u64,
    /// A BFT quorum certificate is attached when the staged BFT driver is
    /// active; the dev/test driver is PoW, so this is honestly `false`.
    pub certified: bool,
}

/// One member's security posture, read from its own node at federation build
/// time: certificate verifier, OCSP staple verification (ADR-017), channel
/// membership and live peer sessions.
#[derive(Serialize, Clone, Default)]
pub struct PostureRow {
    pub company: String,
    pub role: String,
    pub evil: bool,
    /// The node carries a certificate verifier (fail-closed org paths).
    pub verifier: bool,
    /// An X.509 member certificate was issued by the demo Root CA.
    pub certificate: bool,
    /// OCSP staple outcome: issuer-signed and verified locally, or why not.
    pub ocsp: String,
    pub collections: Vec<String>,
    pub peers: u64,
}

/// One registered contract with its real conditions and committed activity.
#[derive(Serialize, Clone, Default)]
pub struct ContractView {
    pub id: String,
    pub buyer: String,
    pub product: String,
    pub max_price_per_unit: u64,
    pub max_quantity: u64,
    pub auto_execute: bool,
    pub executions: u64,
    pub quantity_purchased: u64,
    pub status: String,
}

/// One flat analytical record (the compliance projection of a committed
/// asset registration) for the recent-records table.
#[derive(Serialize, Clone, Default)]
pub struct FlatRecordView {
    pub block: u64,
    pub gtin: String,
    pub batch: String,
    pub serial: String,
    pub custodian: String,
    pub event: String,
    pub trust: u64,
    pub standard: bool,
    pub missing: String,
}

/// Fleet compliance rollup from the leader's projections: schema coverage,
/// standard-compliant vs flagged records, and verifiable-lineage completeness.
#[derive(Serialize, Clone, Default)]
pub struct ComplianceView {
    pub schema_version: String,
    pub fields_present: u64,
    pub fields_total: u64,
    pub compliant: u64,
    pub non_compliant: u64,
    pub critical: u64,
    pub warnings: u64,
    pub flat_records: u64,
    pub standard_records: u64,
    pub low_trust_records: u64,
    pub lineages_checked: u64,
    pub lineages_complete: u64,
    pub avg_trust: u8,
    pub recent: Vec<FlatRecordView>,
}

/// One round's measured point for the performance charts.
#[derive(Serialize, Clone, Default)]
pub struct RoundPoint {
    pub round: u64,
    pub submitted: u64,
    pub rejected: u64,
    pub commit_ms: u64,
    pub pool_count: u64,
    pub tx_per_sec: u64,
    /// Where the round's wall-clock went, in milliseconds: local scenario
    /// production, private-payload dissemination, node submission, pool
    /// settle, PoW mining, chain projections, retail sales.
    pub produce_ms: u64,
    pub payload_ms: u64,
    pub submit_ms: u64,
    pub settle_ms: u64,
    pub project_ms: u64,
    pub retail_ms: u64,
    pub round_ms: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct RunState {
    pub status: String,
    pub round: u64,
    pub chain_height: u64,
    pub lots: Vec<LotView>,
    pub certs: Vec<CertView>,
    pub security: Vec<SecurityEvent>,
    pub offers: Vec<OfferEvent>,
    pub edges: Vec<EdgeCount>,
    pub pdc: Vec<PdcEntry>,
    pub feed: Vec<FeedItem>,
    pub metrics: Metrics,
    pub orgs: Vec<OrgView>,
    pub params: SimParams,
    pub equivocations: Vec<String>,
    /// WMS-like warehouse rows (who holds what, movement totals).
    pub wms: Vec<WmsRow>,
    /// Header KPIs for the WMS section.
    pub wms_summary: WmsSummary,
    /// Recent transactions with their graph edge — the animated dots and
    /// their click-through details.
    pub transactions: Vec<TxView>,
    /// Demo cash bookkeeping per company (minor currency units). Purchases
    /// move cash from buyer to seller. Demo-only state, clearly labelled in
    /// the UI — cash is not an on-chain concept.
    pub cash: BTreeMap<String, u64>,
    /// Sellable inventory per company: what the WMS holds that has not been
    /// sold on. Retail sales drain it slowly — nobody sells through their
    /// whole stock immediately, and draining it fully is possible.
    pub inventory: BTreeMap<String, u64>,
    /// Recent committed blocks — the tamper-evident hash chain.
    pub blocks: Vec<BlockView>,
    /// Per-member security posture (verifier, OCSP staple, channels, peers).
    pub posture: Vec<PostureRow>,
    /// Registered contracts with their real conditions and committed activity.
    pub contracts: Vec<ContractView>,
    /// Compliance rollup: schema coverage, trust distribution, lineage checks.
    pub compliance: ComplianceView,
    /// Per-round measured points for the performance charts (rolling window).
    pub history: Vec<RoundPoint>,
    /// Run start (ms since epoch) for throughput math. Not shown raw.
    #[serde(skip)]
    started_ms: Option<u64>,
    /// Rolling commit latencies (percentile source, not shown raw).
    #[serde(skip)]
    commit_times: Vec<u64>,
    /// Highest block index already folded into the compliance/trust
    /// aggregates (incremental projections; not shown raw).
    #[serde(skip)]
    last_projected_block: u64,
    /// Highest block index already folded into the contract registry.
    #[serde(skip)]
    last_contract_block: u64,
    /// Cumulative trust per originating org: (sum of scores, records).
    #[serde(skip)]
    trust_totals: BTreeMap<String, (u64, u64)>,
    /// Cumulative trust per lot batch: (sum of scores, records).
    #[serde(skip)]
    batch_totals: BTreeMap<u64, (u64, u64)>,
    /// Fleet-wide trust accumulator: (sum of scores, records).
    #[serde(skip)]
    trust_total: (u64, u64),
}

impl RunState {
    fn push_feed(&mut self, label: impl Into<String>) {
        self.feed.push(FeedItem {
            label: label.into(),
            height: self.chain_height,
        });
        if self.feed.len() > FEED_CAP {
            let over = self.feed.len() - FEED_CAP;
            self.feed.drain(0..over);
        }
    }

    fn push_security(&mut self, event: SecurityEvent) {
        self.security.push(event);
        if self.security.len() > SECURITY_CAP {
            let over = self.security.len() - SECURITY_CAP;
            self.security.drain(0..over);
        }
    }

    fn push_offer(&mut self, event: OfferEvent) {
        self.offers.push(event);
        if self.offers.len() > OFFER_CAP {
            let over = self.offers.len() - OFFER_CAP;
            self.offers.drain(0..over);
        }
    }

    fn record_tx(&mut self, id: String, kind: &str, from: &str, to: &str, label: String) {
        self.count_edge(from, to);
        self.transactions.push(TxView {
            id,
            kind: kind.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            label,
            height: self.chain_height,
        });
        if self.transactions.len() > 40 {
            self.transactions.remove(0);
        }
    }

    fn count_edge(&mut self, from: &str, to: &str) {
        match self
            .edges
            .iter_mut()
            .find(|edge| edge.from == from && edge.to == to)
        {
            Some(edge) => edge.count += 1,
            None => self.edges.push(EdgeCount {
                from: from.to_owned(),
                to: to.to_owned(),
                count: 1,
            }),
        }
    }

    fn record_commit(&mut self, commit_ms: u64, pool_count: u64, pool_bytes: u64) {
        self.metrics.blocks += 1;
        self.metrics.last_commit_ms = commit_ms;
        self.metrics.pool_count = pool_count;
        self.metrics.pool_bytes = pool_bytes;
        self.commit_times.push(commit_ms);
        if self.commit_times.len() > COMMIT_WINDOW {
            self.commit_times.remove(0);
        }
        self.metrics.commit_p50_ms = percentile(&self.commit_times, 50);
        self.metrics.commit_p95_ms = percentile(&self.commit_times, 95);
        self.metrics.commit_p99_ms = percentile(&self.commit_times, 99);
    }

    /// Bump one WMS movement counter, creating the row if needed.
    /// Recompute WMS card valuations and the header KPIs. Value is on-hand
    /// × retail price; "low stock" means the sellable pool dropped below
    /// one retail drain (RETAIL_UNITS_PER_ROUND × pressure).
    fn refresh_wms(&mut self) {
        for row in self.wms.iter_mut() {
            row.lots_here = 0;
            row.units_here = 0;
        }
        for lot in &self.lots {
            let Some(last) = lot.chain.last() else {
                continue;
            };
            if let Some(row) = self
                .wms
                .iter_mut()
                .find(|row| row.company == last.custodian)
            {
                row.lots_here += 1;
                row.units_here += 500;
            }
        }
        let pressure = buy_pressure(self);
        let sellable: BTreeMap<String, u64> = self.inventory.clone();
        for row in self.wms.iter_mut() {
            row.stock_value_minor = row.units_here.saturating_mul(RETAIL_PRICE);
            row.sellable_units = sellable.get(&row.company).copied().unwrap_or(0);
        }
        self.wms_summary = WmsSummary {
            members: self.wms.len() as u64,
            total_units: self.wms.iter().map(|row| row.units_here).sum(),
            total_value_minor: self.wms.iter().map(|row| row.stock_value_minor).sum(),
            total_sold_units: self.wms.iter().map(|row| row.sold_units).sum(),
            low_stock: self
                .wms
                .iter()
                .filter(|row| {
                    sellable.get(&row.company).copied().unwrap_or(0)
                        < RETAIL_UNITS_PER_ROUND * pressure
                })
                .count() as u64,
        };
    }

    /// Bump one WMS movement counter, creating the row if needed.
    fn wms_move(&mut self, company: &str, role: &str, received: u64, dispatched: u64) {
        let row = match self.wms.iter_mut().find(|row| row.company == company) {
            Some(row) => row,
            None => {
                self.wms.push(WmsRow {
                    company: company.to_owned(),
                    role: role.to_owned(),
                    product: "SKU-DEMO".into(),
                    lots_here: 0,
                    units_here: 0,
                    received_units: 0,
                    dispatched_units: 0,
                    sold_units: 0,
                    stock_value_minor: 0,
                    sellable_units: 0,
                });
                self.wms.last_mut().expect("row just pushed")
            }
        };
        row.received_units += received;
        row.dispatched_units += dispatched;
        // Sellable inventory moves with the goods: goods-in at each handover
        // adds to the holder's sellable stock.
        if received > 0 {
            *self.inventory.entry(company.to_owned()).or_insert(0) += received;
        }
    }

    /// Record a retail sale: inventory out, the WMS `sold_units` counter
    /// up, and cash in (demo bookkeeping). The sale is a real committed
    /// `InventoryUpdate` on the chain; the graph does not animate it.
    fn retail_sale(&mut self, company: &str, role: &str, units: u64, minor_units: u64) {
        let row = match self.wms.iter_mut().find(|row| row.company == company) {
            Some(row) => row,
            None => return,
        };
        row.sold_units += units;
        *self.inventory.entry(company.to_owned()).or_insert(0) = self
            .inventory
            .get(company)
            .copied()
            .unwrap_or(0)
            .saturating_sub(units);
        *self.cash.entry(company.to_owned()).or_insert(CASH_START) = self
            .cash
            .get(company)
            .copied()
            .unwrap_or(CASH_START)
            .saturating_add(minor_units);
        let _ = role;
    }

    /// Sellable units held by a company (inventory tracked separately from
    /// the committed-custody WMS view).
    fn sellable(&self, company: &str) -> u64 {
        self.inventory.get(company).copied().unwrap_or(0)
    }

    /// Move cash for a committed purchase (demo-only bookkeeping).
    fn cash_move(&mut self, buyer: &str, seller: &str, minor_units: u64) {
        *self.cash.entry(buyer.to_owned()).or_insert(CASH_START) = self
            .cash
            .get(buyer)
            .copied()
            .unwrap_or(CASH_START)
            .saturating_sub(minor_units);
        *self.cash.entry(seller.to_owned()).or_insert(CASH_START) = self
            .cash
            .get(seller)
            .copied()
            .unwrap_or(CASH_START)
            .saturating_add(minor_units);
    }

    fn refresh_throughput(&mut self) {
        let elapsed = self
            .started_ms
            .map_or(0, |started| now_millis().saturating_sub(started) / 1_000);
        self.metrics.elapsed_s = elapsed;
        self.metrics.tx_per_sec = self.metrics.submitted / elapsed.max(1);
    }
}

/// A member org's PDC view, read from **its own node**: what the collection
/// holds on-chain and whether that member's transient store actually carries
/// the cleartext. `payload: None` means the member does not hold it — the
/// honest "not a member / not yet disseminated" case.
#[derive(Serialize, Clone)]
pub struct OrgPayload {
    pub collection: String,
    pub commitment: String,
    pub payload: Option<String>,
    /// The member whose terms this payload carries.
    pub author: String,
    /// The lot the payload belongs to (parsed from the ledger cleartext
    /// server-side — commitments don't carry it in the clear).
    pub lot: u64,
    /// The payload kind (pricing / storage / transit / process / intake /
    /// temperature_log / certification_evidence / regulator_notes).
    pub kind: String,
    /// One-line human summary of the terms, computed server-side so even
    /// commitment rows describe what they hold.
    pub summary: String,
}

/// Server-side summary per payload kind — the drawer renders this directly
/// so commitment rows describe themselves too.
fn payload_summary(terms: &Value) -> String {
    let kind = terms
        .get("kind")
        .and_then(|kind| kind.as_str())
        .unwrap_or("terms");
    match kind {
        "storage" => format!(
            "{} · {} · humidity {} · retain {}d",
            terms
                .get("warehouse")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("temp_range")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("humidity")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("retention_days")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        ),
        "transit" => format!(
            "{} · {} h transit · {} cold chain · {}",
            terms.get("route").and_then(|v| v.as_str()).unwrap_or("—"),
            terms
                .get("transit_hours")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            terms
                .get("cold_chain")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("delivery_window")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
        ),
        "process" => format!(
            "{} · batch {} · {} · shift {}",
            terms
                .get("steps")
                .and_then(|v| v.as_array())
                .map_or("steps".to_owned(), |steps| steps
                    .iter()
                    .filter_map(|step| step.as_str())
                    .collect::<Vec<_>>()
                    .join(" → ")),
            terms
                .get("batch_record")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("gmp_line")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("operator_shift")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
        ),
        "intake" => format!(
            "checks: {} · quarantine {} h",
            terms
                .get("checks")
                .and_then(|v| v.as_array())
                .map(|checks| checks
                    .iter()
                    .filter_map(|check| check.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_else(|| "—".to_owned()),
            terms
                .get("quarantine_hours")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        ),
        "temperature_log" => format!(
            "{} · {}–{} °C · {}/h · {} excursions",
            terms.get("sensor").and_then(|v| v.as_str()).unwrap_or("—"),
            terms.get("min_c").and_then(|v| v.as_f64()).unwrap_or(0.0),
            terms.get("max_c").and_then(|v| v.as_f64()).unwrap_or(0.0),
            terms
                .get("samples_per_hour")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            terms
                .get("excursions")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        ),
        "certification_evidence" => format!(
            "{} · {} samples · lab {} · next audit {}",
            terms
                .get("findings")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("samples_tested")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            terms
                .get("lab_reference")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("next_audit")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
        ),
        "regulator_notes" => format!(
            "{} · finding: {} · {}",
            terms
                .get("inspection")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms.get("finding").and_then(|v| v.as_str()).unwrap_or("—"),
            terms
                .get("inspector")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
        ),
        _ => format!(
            "member ${:.2} · list ${:.2} · {} {} · {}",
            terms
                .get("member_price_per_unit")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 100.0,
            terms
                .get("list_price_per_unit")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 100.0,
            terms.get("quantity").and_then(|v| v.as_u64()).unwrap_or(0),
            terms
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
            terms
                .get("payment_terms")
                .and_then(|v| v.as_str())
                .unwrap_or("—"),
        ),
    }
}

/// Payloads disseminated by the runner: `(collection, commitment,
/// cleartext, author)`. The author is the member whose terms the payload
/// carries — origin-scoped visibility (a member reads its own cleartext;
/// other members' terms stay commitments; the regulator reads everything).
pub type PayloadLedger = Vec<(String, String, String, String)>;

/// A lot in flight: each round it advances exactly one custody hop, so
/// goods physically rest at every member between rounds.
#[derive(Clone)]
pub struct PipelineLot {
    pub seq: u64,
    /// 0 = manufactured, 1 = at distributor, 2 = at logistics, 3 = complete.
    pub stage: u64,
    pub manufacturer: String,
    pub distributor: String,
    pub logistics: String,
    pub pharmacy: String,
    pub certifier: String,
    pub certified: bool,
}

#[derive(Default)]
pub struct SharedRun {
    pub state: Mutex<RunState>,
    /// Per-company node handles: the per-org views read each member's OWN
    /// node (its ledger height, provenance index and transient store).
    pub nodes: Mutex<BTreeMap<String, Arc<Node>>>,
    pub payloads: Mutex<PayloadLedger>,
    pub task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Operator-editable simulation parameters (sanitized on write).
    pub params: Mutex<SimParams>,
    /// Lots in flight: one custody hop per round each.
    pub pipeline: Mutex<Vec<PipelineLot>>,
    /// Bumped on every topology-affecting parameter change; the runner
    /// rebuilds its federation when it sees a newer version.
    pub topology_version: std::sync::atomic::AtomicU64,
}

impl SharedRun {
    /// The serialized public snapshot (leader-derived; no PDC cleartext).
    pub async fn snapshot_value(&self) -> Value {
        let state = self.state.lock().await;
        serde_json::to_value(&*state).unwrap_or_default()
    }
}

type Nodes = BTreeMap<String, Arc<Node>>;

// ── Federation construction ─────────────────────────────────────────────────

async fn verifier_for(org: &Organization) -> CertChainVerifier {
    let mut verifier =
        CertChainVerifier::from_org(org).expect("demo verifier builds from the root org");
    let crl = org.crl_pem().expect("fresh org mints a CRL");
    verifier.add_crl_pem(&crl).expect("the org's own CRL loads");
    verifier
}

/// Build the federation for the given companies: one fresh organization
/// issues every identity (the demo simplification of ADR-011), every
/// verifier-carrying node verifies certificates fail-closed, and each node
/// holds the `pricing` collection config. The star center (the first
/// company) is the block producer; everyone dials it. Returns the nodes and
/// the per-member security posture read from each node once connected.
async fn build_federation(companies: &[Company], leader: &str) -> (Nodes, Vec<PostureRow>) {
    let mut org = Organization::new("GlassChain Demo").expect("demo root org builds");
    let verifier = verifier_for(&org).await;
    let mut nodes: Nodes = BTreeMap::new();
    let mut certificates: BTreeMap<String, String> = BTreeMap::new();

    // Concrete loopback ports: `listen_addr()` echoes the *configured*
    // string, so a wildcard `:0` would make peers dial port 0. Reserve each
    // socket first (the repo's prebound-listener pattern).
    let leader_addr = reserve_addr();
    for company in companies {
        let identity = org
            .issue_identity(company.id.clone())
            .expect("demo identity mints")
            .clone();
        certificates.insert(
            company.id.clone(),
            identity.certificate_pem.clone().unwrap_or_default(),
        );
        let addr = if company.id == leader {
            leader_addr.clone()
        } else {
            reserve_addr()
        };
        let node = Arc::new(Node::new_with_identity(
            &company.id,
            addr,
            1,
            Arc::new(identity),
        ));
        if company.has_verifier {
            node.set_cert_verifier(verifier.clone()).await;
        }
        if company.id == leader {
            node.start(Vec::new()).await.expect("leader binds loopback");
        } else {
            node.start(vec![leader_addr.clone()])
                .await
                .expect("member dials leader");
        }
        nodes.insert(company.id.clone(), node);
    }
    // Every node carries the `pricing` collection config, so the posture read
    // below reports real membership.
    for node in nodes.values() {
        node.set_collections(vec![pricing_collection(companies)])
            .await;
    }
    // Allow the handshake wave + first sync; scale the settle with the fleet.
    tokio::time::sleep(Duration::from_millis(
        500 + 80 * u64::from(companies.len() as u32),
    ))
    .await;

    // Real security posture: mint an issuer-signed OCSP staple per member and
    // verify it locally against the shared verifier (ADR-017), read the
    // node's channel membership and live peer sessions.
    let mut posture = Vec::new();
    for company in companies {
        let cert_pem = certificates.get(&company.id).cloned().unwrap_or_default();
        let ocsp = match org.ocsp_response_der(&company.id) {
            Ok(staple) => match verifier.verify_ocsp_staple(&cert_pem, &staple) {
                Ok(OcspStatus::Good) => "issuer-signed · verified locally".to_owned(),
                Ok(OcspStatus::Revoked) => "revoked — session fails closed".to_owned(),
                Err(error) => format!("unverified: {error}"),
            },
            Err(error) => format!("not minted: {error}"),
        };
        let node = nodes.get(&company.id).expect("node just inserted");
        posture.push(PostureRow {
            company: company.id.clone(),
            role: company.role.clone(),
            evil: company.evil,
            verifier: company.has_verifier,
            certificate: !cert_pem.is_empty(),
            ocsp,
            collections: node.collection_names().await,
            peers: node.known_peers().await.len() as u64,
        });
    }
    (nodes, posture)
}

/// Reserve a concrete loopback address with its socket parked so `Node::start`
/// adopts the same socket (no `AddrInUse` window, no `:0` dial bug).
fn reserve_addr() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("demo binds loopback");
    let addr = listener
        .local_addr()
        .expect("bound addr has a port")
        .to_string();
    glasschain_network::stash_prebound_listener(&addr, listener);
    addr
}

/// `pricing` collection membership: every honest supply-chain company plus
/// the regulators. Certifiers and evil companies are deliberately excluded.
pub fn pricing_member_ids(companies: &[Company]) -> Vec<String> {
    companies
        .iter()
        .filter(|company| !company.evil && company.role != "certifier")
        .map(|company| company.id.clone())
        .collect()
}

fn pricing_collection(companies: &[Company]) -> Channel {
    Channel::new(ChannelConfig {
        name: COLLECTION.to_owned(),
        member_ids: pricing_member_ids(companies),
        description: "demo pricing payloads (member-only)".into(),
        endorsement_policy: None,
        retention_secs: 3600,
    })
}

// ── Transaction builders ────────────────────────────────────────────────────

fn percentile(samples: &[u64], pct: u8) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut owned = samples.to_vec();
    owned.sort_unstable();
    let idx = ((owned.len() as u64 * u64::from(pct)) / 100).min(owned.len() as u64 - 1);
    owned[idx as usize]
}

fn activation_tx(height: u64) -> Transaction {
    Transaction::with_id(
        format!("cap:pdc:{height}"),
        TransactionKind::CapabilityActivation(CapabilityActivation {
            capability_id: "pdc".into(),
            version: 1,
            hash: capability_hash("pdc", 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
                signer: "gov".into(),
                signature_bytes: vec![0x42],
            }],
        }),
    )
}

/// A contract definition with its buyer, price ceiling, lifetime cap and
/// execution mode. One auto contract plus one conditions-only contract per
/// pharmacy: price bands never overlap between autos, so a single offer can
/// only ever produce one automatic purchase.
fn contract_tx(
    id: &str,
    buyer: &str,
    max_price: u64,
    max_quantity: u64,
    auto_execute: bool,
    max_lead_time_days: u32,
) -> Transaction {
    Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
        contract_id: id.into(),
        buyer_id: buyer.into(),
        product_id: "SKU-DEMO".into(),
        conditions: PurchaseConditions {
            max_price_per_unit: max_price,
            min_quantity: 1,
            max_quantity,
            max_lead_time_days,
            preferred_seller_id: None,
            currency: "USD".into(),
            auto_execute,
        },
        wasm_code_b64: None,
    }))
}

/// The offer price ladder: cheap rungs under the auto cap, and every third
/// offer a premium rung on one of the three manual bands (each owned by a
/// different pharmacy). Prices and quantities vary per lot.
fn offer_price(seq: u64) -> u64 {
    if seq.is_multiple_of(3) {
        MANUAL_PRICE_TIERS[((seq / 3) % MANUAL_PRICE_TIERS.len() as u64) as usize]
    } else {
        AUTO_PRICE_TIERS[((seq.wrapping_mul(5) + seq / 4) % AUTO_PRICE_TIERS.len() as u64) as usize]
    }
}

/// Each lot is 500 units; the offer covers a varied slice of it. The
/// stock-driven buy pressure adds a step, so scarce stock keeps partial
/// offers and piled-up stock approaches full lots.
fn offer_quantity(seq: u64, pressure: u64) -> u64 {
    let tiers: &[u64] = if offer_price(seq) > CONTRACT_MAX_PRICE {
        &MANUAL_QUANTITY_TIERS
    } else {
        &AUTO_QUANTITY_TIERS
    };
    let base = tiers[((seq.wrapping_mul(7) + seq / 5) % tiers.len() as u64) as usize];
    (base + pressure * 50).min(500)
}

/// The conditions-only contract an offer above the auto cap matches, by
/// price band. Bands are disjoint, so a premium offer never matches more
/// than one contract.
fn manual_contract_for_price(price: u64) -> &'static str {
    if price > MANUAL_MAX_PRICE {
        PREMIUM_CONTRACT_ID
    } else if price > VALUE_MAX_PRICE {
        MANUAL_CONTRACT_ID
    } else {
        VALUE_CONTRACT_ID
    }
}

/// The buyer that owns the conditions-only contract for a price band (or the
/// auto contract's buyer under the cap).
fn offer_buyer(price: u64) -> &'static str {
    match manual_contract_for_price(price) {
        PREMIUM_CONTRACT_ID => "pharmacy-3",
        MANUAL_CONTRACT_ID => "pharmacy-1",
        VALUE_CONTRACT_ID => "pharmacy-2",
        _ => "pharmacy-1",
    }
}

/// The conditions-only contract a manual purchase as `buyer` completes
/// against: each pharmacy's own.
fn manual_contract_for_buyer(buyer: &str) -> &'static str {
    match buyer {
        "pharmacy-2" => VALUE_CONTRACT_ID,
        "pharmacy-3" => PREMIUM_CONTRACT_ID,
        _ => MANUAL_CONTRACT_ID,
    }
}

/// Absolute system stock: every company's sellable inventory summed.
fn system_stock(state: &RunState) -> u64 {
    state.inventory.values().sum()
}

/// The stock-driven purchase multiplier (1 = scarce, PRESSURE_CAP = flush).
fn buy_pressure(state: &RunState) -> u64 {
    (1 + system_stock(state) / STOCK_PRESSURE_DIVISOR).min(PRESSURE_CAP)
}

fn supply_offer(seq: u64, seller: &str, pressure: u64) -> Transaction {
    // Every third offer is priced above the `auto-replenish` cap (1 200) but
    // inside `manual-review`'s ceiling: it is advertised and matched, yet
    // waits for a human buyer to complete the purchase.
    let price_per_unit = offer_price(seq);
    Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
        product_id: "SKU-DEMO".into(),
        product_name: "Demo Vaccine 10-Dose".into(),
        seller_id: seller.into(),
        quantity_available: offer_quantity(seq, pressure),
        price_per_unit,
        lead_time_days: 3 + u32::try_from(seq % 4).unwrap_or(0),
        currency: "USD".into(),
    }))
}

fn lot_record(seq: u64, manufacturer: &str) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert("lot_id".to_owned(), Value::String(format!("LOT-{seq}")));
    payload.insert("product_id".to_owned(), Value::String("SKU-DEMO".into()));
    payload.insert(
        "batch_number".to_owned(),
        Value::String(format!("B-{seq:04}")),
    );
    let mut record = CanonicalRecord::new(1_700_000_000, "lot", payload, manufacturer);
    record.record_id = format!("lot-{seq}");
    record.commitment = record.commitment().ok();
    record
}

fn signed(record: CanonicalRecord, signer: &str) -> Transaction {
    let mut staged = record;
    staged.signatures.push(RecordSignature {
        algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
        signer: signer.to_owned(),
        signature_bytes: vec![0x42],
    });
    Transaction::with_id(
        staged.record_id.clone(),
        TransactionKind::CanonicalRecord(staged),
    )
}

fn cert_record(seq: u64, schema: &str, record_id: String, issuer: &str) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert("lot_ref".to_owned(), Value::String(format!("lot-{seq}")));
    payload.insert("issuer".to_owned(), Value::String(issuer.into()));
    payload.insert("scope".to_owned(), Value::String("demo scope".into()));
    payload.insert("valid_from".to_owned(), Value::String("2026-09-16".into()));
    payload.insert("valid_to".to_owned(), Value::String("2027-09-16".into()));
    payload.insert("status".to_owned(), Value::String("valid".into()));
    let mut evidence = serde_json::Map::new();
    evidence.insert(
        "manifest_commitment".to_owned(),
        Value::String(format!("{seq:064x}")),
    );
    payload.insert("evidence_manifest".to_owned(), Value::Object(evidence));
    let mut record = CanonicalRecord::new(1_700_000_000, schema, payload, issuer);
    record.record_id = record_id;
    record.commitment = record.commitment().ok();
    record
}

fn asset(custodian_id: &str, seq: u64, evil_metadata: bool) -> TraceableAsset {
    let mut asset = TraceableAsset {
        gtin: Some(GTIN.into()),
        batch_number: Some(format!("B-{seq:04}")),
        expiry_date: Some("2027-06-30".into()),
        serial_number: Some(format!("SN-BR-{seq:06}")),
        anvisa_registration: Some("MS 1.1234.0042.001-7".into()),
        manufacturer_id: Some("12.345.678/0001-99 (fictional)".into()),
        product_name: "Demo Vaccine 10-Dose".into(),
        custodian_id: custodian_id.into(),
        country_of_origin: Some("BR".into()),
        storage_temp_celsius: Some("2-8".into()),
        quantity: 500,
    };
    if evil_metadata {
        // The evil company's metadata is non-standard: no serial number. The
        // registration is still admitted (zero trust is not exclusion) but
        // the trust score drops and the shortfall is visible in the UI.
        asset.serial_number = None;
        asset.anvisa_registration = None;
        asset.manufacturer_id = None;
    }
    asset
}

fn custody_tx(seq: u64, event_type: &str, custodian: &str, evil_metadata: bool) -> Transaction {
    Transaction::new(TransactionKind::AssetRegistration(
        TraceableAssetRegistration {
            asset: asset(custodian, seq, evil_metadata),
            event_type: event_type.into(),
            // The registering member is the originator, so per-org trust can
            // be judged from committed registrations (evil metadata scores low).
            originator_id: custodian.into(),
            purchase_order_ref: None,
        },
    ))
}

/// The member-only pricing terms for a lot: the public `SupplyOffer`
/// carries the list price on-chain; the private payload carries the
/// negotiated terms (a member discount), disseminated point-to-point.
/// Written from the star center so every collection member holds it (the
/// engine disseminates one hop).
/// The private payloads each role adds for a lot — every collection member
/// contributes its OWN class of terms, origin-scoped. Written from the
/// star center (the engine disseminates one hop).
fn pricing_terms(seq: u64, pressure: u64, manufacturer: &str) -> String {
    let list_price = offer_price(seq);
    let member_price = list_price - list_price / 8; // 12.5 % member discount
    serde_json::json!({
        "kind": "pricing",
        "lot": seq,
        "author": manufacturer,
        "list_price_per_unit": list_price,
        "member_price_per_unit": member_price,
        "quantity": offer_quantity(seq, pressure),
        "currency": "USD",
        "payment_terms": "net 60",
    })
    .to_string()
}

/// Storage terms: the distributor's warehouse commitments for the lot.
fn storage_terms(seq: u64, distributor: &str) -> String {
    serde_json::json!({
        "kind": "storage",
        "lot": seq,
        "author": distributor,
        "warehouse": format!("DC-{}", distributor),
        "temp_range": "2-8°C",
        "humidity": "≤60%",
        "retention_days": 90,
        "lot_release": "on certification",
    })
    .to_string()
}

/// Transit terms: the logistics carrier's delivery commitments.
fn transit_terms(seq: u64, logistics: &str) -> String {
    serde_json::json!({
        "kind": "transit",
        "lot": seq,
        "author": logistics,
        "carrier": logistics,
        "route": "mfg → hub → pharmacy",
        "transit_hours": 36,
        "cold_chain": "continuous",
        "delivery_window": "next business day",
    })
    .to_string()
}

/// Process terms: the manufacturer's internal manufacturing steps — no
/// member-to-member transaction, just the maker recording its own process.
fn process_terms(seq: u64, manufacturer: &str) -> String {
    serde_json::json!({
        "kind": "process",
        "lot": seq,
        "author": manufacturer,
        "steps": ["mixing", "filling", "packaging"],
        "batch_record": format!("BR-{seq:04}"),
        "gmp_line": format!("line-{}", (seq % 3) + 1),
        "operator_shift": "B",
    })
    .to_string()
}

/// Intake checklist: the distributor's receiving inspection commitments.
fn intake_checklist(seq: u64, distributor: &str) -> String {
    serde_json::json!({
        "kind": "intake",
        "lot": seq,
        "author": distributor,
        "checks": ["seal integrity", "cold-chain log", "quantity match"],
        "quarantine_hours": 24,
        "accepted_by": distributor,
    })
    .to_string()
}

/// Temperature log: the logistics carrier's continuous cold-chain record.
fn temperature_log_terms(seq: u64, logistics: &str) -> String {
    serde_json::json!({
        "kind": "temperature_log",
        "lot": seq,
        "author": logistics,
        "sensor": format!("TS-{seq:04}"),
        "min_c": 2.0,
        "max_c": 7.5,
        "samples_per_hour": 4,
        "excursions": 0,
    })
    .to_string()
}

/// Certifier evidence: the private half of the quality certification — the
/// raw inspection findings stay off-chain while the public record anchors.
fn certifier_evidence(seq: u64, certifier: &str) -> String {
    serde_json::json!({
        "kind": "certification_evidence",
        "lot": seq,
        "author": certifier,
        "inspector": format!("{certifier} field team"),
        "findings": "batch conforms to GxP — no deviations observed",
        "samples_tested": 12,
        "lab_reference": format!("LAB-{seq:04}"),
        "next_audit": "2027-03-16",
    })
    .to_string()
}

/// Regulator compliance notes: the regulator's own private assessment of a
/// lot — visible only to the regulator and Admin.
fn regulator_notes(seq: u64) -> String {
    serde_json::json!({
        "kind": "regulator_notes",
        "lot": seq,
        "author": "regulator-1",
        "inspection": "routine market surveillance",
        "finding": "no deviations",
        "follow_up_required": false,
        "inspector": "AnvisaDemo field officer",
    })
    .to_string()
}

/// A retail sale: the pharmacy sells `units` to end customers out of its
/// sellable stock, registered as a real committed `InventoryUpdate` (a
/// negative stock delta on the chain). The live graph does not animate
/// these — they are the quiet drain on the WMS inventory.
fn retail_sale_tx(company: &str, units: u64, seq: u64) -> Transaction {
    Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
        product_id: "SKU-DEMO".into(),
        owner_id: company.into(),
        quantity_delta: -i64::try_from(units).unwrap_or(i64::MIN),
        reason: format!("retail sale to customers (batch {seq})"),
    }))
}

fn asset_id(seq: u64, evil_metadata: bool) -> String {
    if evil_metadata {
        // No serial → the batch-level id (matches the indexer's fallback).
        return format!("GTIN:{GTIN}:BATCH:B-{seq:04}");
    }
    format!("GTIN:{GTIN}:SN:SN-BR-{seq:06}")
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Manual transactions (the human completes a purchase) ────────────────────

/// Complete a pending offer manually: the buyer submits a real
/// `PurchaseOrder` through its own node (admission, relay, commit — the
/// same path as everything else). `buyer` must be an honest pharmacy.
pub async fn complete_purchase(
    shared: &SharedRun,
    offer_tx_id: &str,
    buyer: &str,
    quantity: u64,
) -> Result<String, String> {
    let pending = {
        let state = shared.state.lock().await;
        state
            .offers
            .iter()
            .find(|event| {
                event.kind == "offer"
                    && event.tx_id == offer_tx_id
                    && (event.note.contains("awaiting") || event.note.contains("still on offer"))
            })
            .cloned()
    };
    let Some(offer) = pending else {
        return Err("offer not found or not awaiting a decision".into());
    };
    let remaining = offer.quantity - offer.sold.min(offer.quantity);
    let quantity = quantity.clamp(1, remaining.max(1));
    let seller = offer.seller.clone();
    let order = Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder {
        product_id: offer.product.clone(),
        buyer_id: buyer.to_owned(),
        seller_id: seller.clone(),
        quantity,
        agreed_price_per_unit: offer.price_per_unit,
        currency: "USD".into(),
        contract_id: Some(manual_contract_for_buyer(buyer).into()),
    }));
    let order_id = order.id.clone();
    let node = node_for(shared, buyer).await;
    node.submit_transaction(order)
        .await
        .map_err(|error| format!("admission rejected: {error}"))?;
    {
        let mut state = shared.state.lock().await;
        state.cash_move(
            buyer,
            &seller,
            quantity.saturating_mul(offer.price_per_unit),
        );
        let remaining = remaining - quantity;
        let sold = offer.sold + quantity;
        let note = if remaining == 0 {
            format!(
                "fully bought by manual purchases ({sold} of {} units)",
                offer.quantity
            )
        } else {
            format!(
                "partially bought: {sold} of {} units, {remaining} still on offer",
                offer.quantity
            )
        };
        if let Some(row) = state
            .offers
            .iter_mut()
            .find(|event| event.kind == "offer" && event.tx_id == offer_tx_id)
        {
            row.sold = sold;
            row.note = note;
        }
        let round = state.round;
        state.push_offer(OfferEvent {
            kind: "purchase".into(),
            tx_id: order_id.clone(),
            seller,
            buyer: buyer.to_owned(),
            product: offer.product.clone(),
            quantity,
            sold: 0,
            price_per_unit: offer.price_per_unit,
            round,
            note: format!(
                "manually completed by {buyer} against {contract} (committing next block)",
                contract = manual_contract_for_buyer(buyer)
            ),
        });
        state.record_tx(
            order_id.clone(),
            "purchase",
            &offer.seller,
            buyer,
            format!(
                "Manual PurchaseOrder {quantity} × {} @ {} by {buyer}",
                offer.product, offer.price_per_unit
            ),
        );
    }
    Ok(order_id)
}

// ── Run control (parameters apply live, topology rebuilds when needed) ──────

/// Validate + store new parameters. Topology-affecting counts bump the
/// topology version; the runner rebuilds its federation at the next round
/// boundary without dropping the run or its counters.
pub async fn apply_params(shared: &SharedRun, requested: SimParams) -> SimParams {
    let sanitized = requested.sanitized();
    let mut current = shared.params.lock().await;
    let topology_changed = current.manufacturers != sanitized.manufacturers
        || current.distributors != sanitized.distributors
        || current.logistics != sanitized.logistics
        || current.pharmacies != sanitized.pharmacies
        || current.regulators != sanitized.regulators
        || current.certifiers != sanitized.certifiers
        || current.evil_nodes != sanitized.evil_nodes;
    *current = sanitized.clone();
    if topology_changed {
        shared
            .topology_version
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    sanitized
}

/// Read-only params echo for the bridge (bootstrap).
pub async fn params_for_view(shared: &SharedRun) -> SimParams {
    params_snapshot(shared).await
}

async fn params_snapshot(shared: &SharedRun) -> SimParams {
    shared.params.lock().await.clone()
}

/// Settle wait: relay the round's submissions into the star center's pool.
/// Waits on the round's exact transaction ids (pool depth alone is fooled by
/// extra traffic), bounded so a slow relay delays the block instead of
/// losing it.
async fn wait_for_ids(leader: &Arc<Node>, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let mut missing: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
    let deadline = Instant::now() + Duration::from_millis(2_500);
    loop {
        let present: std::collections::HashSet<String> = {
            let ledger = leader.shared_ledger();
            let ledger = ledger.lock().await;
            ledger
                .pending_transactions
                .iter()
                .map(|tx| tx.id.clone())
                .collect()
        };
        missing.retain(|id| !present.contains(*id));
        if missing.is_empty() || Instant::now() > deadline {
            return;
        }
        // Tight poll: the relay hop is local loopback, so settle latency is
        // measured in single-digit milliseconds, not ticks.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn ms_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Submit a round's transactions concurrently (bounded): admission is
/// independent per transaction, so the runner fans it out and folds the
/// counters once from the aggregated results.
const SUBMIT_CONCURRENCY: usize = 24;

async fn submit_round(shared: &SharedRun, txs: Vec<(Arc<Node>, Transaction)>) {
    if txs.is_empty() {
        return;
    }
    let mut set = tokio::task::JoinSet::new();
    let mut submitted = 0u64;
    let mut rejected = 0u64;
    let mut failures: Vec<String> = Vec::new();
    for (node, tx) in txs {
        set.spawn(async move { node.submit_transaction(tx).await });
        if set.len() >= SUBMIT_CONCURRENCY {
            if let Some(joined) = set.join_next().await {
                match joined {
                    Ok(Ok(())) => submitted += 1,
                    Ok(Err(error)) => {
                        rejected += 1;
                        failures.push(error.to_string());
                    }
                    Err(_) => rejected += 1,
                }
            }
        }
    }
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => submitted += 1,
            Ok(Err(error)) => {
                rejected += 1;
                failures.push(error.to_string());
            }
            Err(_) => rejected += 1,
        }
    }
    let mut state = shared.state.lock().await;
    state.metrics.submitted += submitted;
    state.metrics.rejected += rejected;
    for error in failures {
        state.push_feed(format!("admission rejected: {error}"));
    }
}

/// Disseminate the round's private payloads concurrently (bounded) and mirror
/// only the ones that actually left the node in the demo ledger — the ledger
/// holds real dissemination, not attempts. Returns `(kind, commitment,
/// cleartext, author)` per accepted payload, in round order.
async fn disseminate_payloads(
    leader: &Arc<Node>,
    payloads: Vec<(String, String)>,
) -> Vec<(String, String, String, String)> {
    const DISSEMINATION_CONCURRENCY: usize = 16;
    let mut accepted = Vec::new();
    for chunk in payloads.chunks(DISSEMINATION_CONCURRENCY) {
        let mut handles = Vec::with_capacity(chunk.len());
        for (offset, (cleartext, author)) in chunk.iter().enumerate() {
            let node = Arc::clone(leader);
            let cleartext = cleartext.clone();
            let author = author.clone();
            handles.push(tokio::spawn(async move {
                let ok = node
                    .submit_private_payload(COLLECTION, cleartext.as_bytes().to_vec())
                    .await
                    .is_ok();
                (offset, cleartext, author, ok)
            }));
        }
        let mut results = Vec::with_capacity(chunk.len());
        for handle in handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
        }
        results.sort_by_key(|(offset, _, _, _)| *offset);
        for (_, cleartext, author, ok) in results {
            if !ok {
                continue;
            }
            let kind = serde_json::from_str::<Value>(&cleartext)
                .ok()
                .and_then(|terms| {
                    terms
                        .get("kind")
                        .and_then(|kind| kind.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "terms".to_owned());
            accepted.push((
                kind,
                glasschain_core::crypto::sha256(cleartext.as_bytes()).to_string(),
                cleartext,
                author,
            ));
        }
    }
    accepted
}

async fn node_for(shared: &SharedRun, id: &str) -> Arc<Node> {
    shared
        .nodes
        .lock()
        .await
        .get(id)
        .cloned()
        .expect("company node exists")
}

/// Common setup: build the federation for the current parameters, configure
/// collections, mark the run running, activate `pdc`, register the
/// auto-replenish contract, and mine the setup block. Returns the leader.
async fn setup_run(shared: &SharedRun) -> Arc<Node> {
    let params = params_snapshot(shared).await;
    let companies = build_companies(&params);
    let leader_id = companies[0].id.clone();
    let (nodes, posture) = build_federation(&companies, &leader_id).await;
    {
        let mut state = shared.state.lock().await;
        state.status = "running".into();
        state.started_ms = Some(now_millis());
        state.params = params.clone();
        state.posture = posture;
        // A rebuilt federation starts from a fresh chain, so the projection
        // cursor and its cumulative aggregates restart with it.
        state.compliance = ComplianceView {
            schema_version: format!("v{SCHEMA_VERSION_V1}"),
            fields_total: 6,
            ..ComplianceView::default()
        };
        state.last_projected_block = 0;
        state.last_contract_block = 0;
        state.trust_totals.clear();
        state.batch_totals.clear();
        state.trust_total = (0, 0);
        state.contracts.clear();
        state.orgs = companies
            .iter()
            .map(|company| OrgView {
                id: company.id.clone(),
                role: company.role.clone(),
                evil: company.evil,
                member_of: if company.evil || company.role == "certifier" {
                    Vec::new()
                } else {
                    vec![COLLECTION.into()]
                },
                trust_score: 0,
                records: 0,
            })
            .collect();
    }
    *shared.nodes.lock().await = nodes;
    let leader = node_for(shared, &leader_id).await;
    let activation = activation_tx(2);
    let _ = leader.submit_transaction(activation).await;
    // The buy side: one auto-executing contract under the auto cap, plus one
    // conditions-only contract per pharmacy, each with its own price band —
    // a premium offer matches exactly one of them and waits for that buyer.
    for contract in [
        contract_tx(
            CONTRACT_ID,
            "pharmacy-1",
            CONTRACT_MAX_PRICE,
            100_000,
            true,
            14,
        ),
        contract_tx(
            MANUAL_CONTRACT_ID,
            "pharmacy-1",
            MANUAL_MAX_PRICE,
            4_000,
            false,
            30,
        ),
        contract_tx(
            VALUE_CONTRACT_ID,
            "pharmacy-2",
            VALUE_MAX_PRICE,
            3_000,
            false,
            21,
        ),
        contract_tx(
            PREMIUM_CONTRACT_ID,
            "pharmacy-3",
            PREMIUM_MAX_PRICE,
            5_000,
            false,
            45,
        ),
    ] {
        let _ = leader.submit_transaction(contract).await;
    }
    // They must be committed before private payloads are legal, so settle
    // them in their own block here.
    let _ = leader.mine().await;
    leader
}

// ── The round loop ──────────────────────────────────────────────────────────

/// One synchronized round: `lots_per_round` lots across rotating real
/// companies, supply offers matched by the real contract engine, both evil
/// companies attacking, one block per round.
async fn drive_round(shared: &SharedRun, leader: &Arc<Node>, round: u64) {
    let params = params_snapshot(shared).await;
    let companies = build_companies(&params);
    // Buy pressure scales with the system's absolute stock: the more units
    // sit in the system, the bigger the contract's purchases (full lots),
    // pushing goods toward the retail drain instead of piling them up.
    let pressure = {
        let state = shared.state.lock().await;
        buy_pressure(&state)
    };
    let manufacturers = companies_of_role(&companies, "manufacturer");
    let distributors = companies_of_role(&companies, "distributor");
    let logistics_list = companies_of_role(&companies, "logistics");
    let pharmacies = companies_of_role(&companies, "pharmacy");
    let certifiers = companies_of_role(&companies, "certifier");
    let round_start = Instant::now();
    let produce_start = Instant::now();
    let mut round_txs: Vec<(Arc<Node>, Transaction)> = Vec::new();
    let mut round_payloads: Vec<(String, String)> = Vec::new();

    // ── 1. Certification + audit for lots already on the chain: the GxP
    // batch certification applies to the MANUFACTURED batch — it is not
    // tied to the pharmacy or any single custody stage.
    {
        let mut pipeline = shared.pipeline.lock().await;
        for entry in pipeline.iter_mut() {
            if !entry.certified {
                let record = cert_record(
                    entry.seq,
                    "quality_certification",
                    format!("cert-{}", entry.seq),
                    &entry.certifier,
                );
                round_txs.push((
                    node_for(shared, &entry.certifier).await,
                    signed(record, &entry.certifier),
                ));
                let audit = cert_record(
                    entry.seq,
                    "audit_attestation",
                    format!("audit-{}", entry.seq),
                    &entry.certifier,
                );
                round_txs.push((
                    node_for(shared, &entry.certifier).await,
                    signed(audit, &entry.certifier),
                ));
                let mut state = shared.state.lock().await;
                state.certs.push(CertView {
                    record_id: format!("cert-{}", entry.seq),
                    schema: "quality_certification".into(),
                    lot_ref: format!("LOT-{}", entry.seq),
                    issuer: entry.certifier.clone(),
                    status: "valid".into(),
                });
                state.certs.push(CertView {
                    record_id: format!("audit-{}", entry.seq),
                    schema: "audit_attestation".into(),
                    lot_ref: format!("LOT-{}", entry.seq),
                    issuer: entry.certifier.clone(),
                    status: "valid".into(),
                });
                if state.certs.len() > 30 {
                    state.certs.remove(0);
                }
                // The certifier's private evidence and the regulator's
                // private compliance notes ride the same round, each
                // authored by its org.
                round_payloads.push((
                    certifier_evidence(entry.seq, &entry.certifier),
                    entry.certifier.clone(),
                ));
                if params.regulators > 0 {
                    round_payloads.push((regulator_notes(entry.seq), "regulator-1".to_owned()));
                }
                entry.certified = true;
            }
        }
    }

    // ── 2. Advance the pipeline: every in-flight lot moves ONE custody hop
    // per round, so goods genuinely rest at each member between rounds.
    {
        let mut pipeline = shared.pipeline.lock().await;
        for entry in pipeline.iter_mut() {
            if entry.stage >= 3 {
                continue;
            }
            let hops: [(&str, &str, &str); 3] = [
                (
                    "dispatch",
                    entry.manufacturer.as_str(),
                    entry.distributor.as_str(),
                ),
                (
                    "dispatch",
                    entry.distributor.as_str(),
                    entry.logistics.as_str(),
                ),
                ("receive", entry.logistics.as_str(), entry.pharmacy.as_str()),
            ];
            let (stage, from, to) = hops[entry.stage as usize];
            let tx = custody_tx(entry.seq, stage, to, false);
            let tx_id = tx.id.clone();
            round_txs.push((node_for(shared, to).await, tx));
            // The taking member contributes its OWN class of private terms
            // for the lot: the distributor storage terms, logistics the
            // transit terms. Disseminated from the star center.
            let terms_payloads: Vec<String> = match entry.stage {
                0 => vec![
                    storage_terms(entry.seq, to),
                    intake_checklist(entry.seq, to),
                ],
                _ => vec![
                    transit_terms(entry.seq, to),
                    temperature_log_terms(entry.seq, to),
                ],
            };
            for terms in terms_payloads {
                round_payloads.push((terms, to.to_owned()));
            }
            {
                let mut state = shared.state.lock().await;
                state.record_tx(
                    tx_id.clone(),
                    "custody",
                    from,
                    to,
                    format!("AssetRegistration `{stage}` → {to} (lot {})", entry.seq),
                );
                state.wms_move(from, "holder", 0, 500);
                state.wms_move(to, "holder", 500, 0);
                state.push_feed(format!(
                    "lot {} handed over: {} → {} ({} units)",
                    entry.seq,
                    from,
                    to,
                    offer_quantity(entry.seq, pressure)
                ));
            }
            entry.stage += 1;
        }
    }

    // ── 3. New lots enter the pipeline: anchor + manufacture this round;
    // downstream hops follow on later rounds.
    for slot in 0..params.lots_per_round as u64 {
        let seq = (round - 1) * u64::from(params.lots_per_round) + slot + 1;
        let manufacturer = manufacturers[(slot as usize) % manufacturers.len()];
        let distributor = distributors[(slot as usize) % distributors.len()];
        let logistics = logistics_list[(slot as usize) % logistics_list.len()];
        let pharmacy = pharmacies[(slot as usize) % pharmacies.len()];
        let certifier = certifiers[(slot as usize) % certifiers.len()];

        {
            let mut state = shared.state.lock().await;
            state.push_feed(format!(
                "lot {seq}: {manufacturer} manufactures; {distributor} → {logistics} → \
                 {pharmacy} queued"
            ));
            state.count_edge(manufacturer, distributor);
            state.count_edge(certifier, pharmacy);
        }

        // Lot anchor + manufacture registration, both in this round's block.
        round_txs.push((
            node_for(shared, manufacturer).await,
            signed(lot_record(seq, manufacturer), manufacturer),
        ));
        let manufacture = custody_tx(seq, "manufacture", manufacturer, false);
        {
            let mut state = shared.state.lock().await;
            state.record_tx(
                manufacture.id.clone(),
                "custody",
                manufacturer,
                manufacturer,
                format!("AssetRegistration `manufacture` → {manufacturer} (lot {seq})"),
            );
            // Goods-in at the maker: the sellable pool grows at handover.
            state.wms_move(manufacturer, "manufacturer", 500, 0);
        }
        round_txs.push((node_for(shared, manufacturer).await, manufacture));
        // Intermediate manufacturing steps: same-member InventoryUpdates —
        // no transaction between members, the maker records its own process
        // on the chain (mixing → packaging).
        for step in ["mixing", "packaging"] {
            let step_tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
                product_id: "SKU-DEMO".into(),
                owner_id: manufacturer.into(),
                quantity_delta: 0,
                reason: format!("manufacturing step: {step} (lot {seq})"),
            }));
            {
                round_txs.push((node_for(shared, manufacturer).await, step_tx.clone()));
                let mut state = shared.state.lock().await;
                state.record_tx(
                    step_tx.id.clone(),
                    "process",
                    manufacturer,
                    manufacturer,
                    format!("Process step `{step}` by {manufacturer} (lot {seq})"),
                );
            }
        }
        // The maker's private process payload joins the pricing terms.
        round_payloads.push((process_terms(seq, manufacturer), manufacturer.to_owned()));

        // The sell side: a SupplyOffer submitted by the manufacturer; the
        // buyer-side contract engine on that node matches it. Cheap offers
        // auto-execute under the cap; every third is premium, matches exactly
        // one pharmacy's conditions-only contract by its price band, and
        // waits for that human buyer.
        let price = offer_price(seq);
        let quantity = offer_quantity(seq, pressure);
        let seller_buyer = offer_buyer(price);
        let contract = if price > CONTRACT_MAX_PRICE {
            manual_contract_for_price(price)
        } else {
            CONTRACT_ID
        };
        let offer_tx = supply_offer(seq, manufacturer, pressure);
        let offer_tx_id = offer_tx.id.clone();
        round_txs.push((node_for(shared, manufacturer).await, offer_tx));
        {
            let mut state = shared.state.lock().await;
            state.push_offer(OfferEvent {
                kind: "offer".into(),
                tx_id: offer_tx_id.clone(),
                seller: manufacturer.to_owned(),
                buyer: seller_buyer.to_owned(),
                product: "SKU-DEMO".into(),
                quantity,
                sold: 0,
                price_per_unit: price,
                round,
                note: if price > CONTRACT_MAX_PRICE {
                    format!(
                        "matched {contract} for {seller_buyer} — {quantity} of the lot's 500 units on offer, awaiting a buyer decision"
                    )
                } else {
                    format!(
                        "partial lot offer: {quantity} of the lot's 500 units (auto contract)"
                    )
                },
            });
            state.record_tx(
                offer_tx_id,
                "offer",
                manufacturer,
                seller_buyer,
                format!("SupplyOffer {seq} from {manufacturer}"),
            );
            state.count_edge(manufacturer, seller_buyer);
        }

        // Member-only pricing payload from the star center — the block
        // holds only its commitment, the cleartext is disseminated to the
        // collection members. (The engine relays dissemination one hop, so
        // the star center is the writer that reaches every member.)
        round_payloads.push((
            pricing_terms(seq, pressure, manufacturer),
            manufacturer.to_owned(),
        ));

        {
            let mut state = shared.state.lock().await;
            let trust = MetadataTrustScore::compute(&asset(manufacturer, seq, false));
            state.lots.push(LotView {
                lot_ref: format!("LOT-{seq}"),
                status: "manufactured".into(),
                manufacturer: manufacturer.to_owned(),
                trust_score: trust.score,
                chain: Vec::new(),
                lineage_complete: false,
                trust_avg: trust.score,
                flat_records: 0,
                schema_compliant: true,
            });
            state.metrics.lots += 1;
        }
        shared.pipeline.lock().await.push(PipelineLot {
            seq,
            stage: 0,
            manufacturer: manufacturer.to_owned(),
            distributor: distributor.to_owned(),
            logistics: logistics.to_owned(),
            pharmacy: pharmacy.to_owned(),
            certifier: certifier.to_owned(),
            certified: false,
        });
    }

    // ── 4. Evil attempts: recorded outcomes, whatever the node decides.
    // Only THIS round's submissions are awaited in the settle step — waiting
    // on pipeline anchor ids from earlier rounds pinned every round to the
    // 2.5 s deadline (they were committed long ago and can never be pending).
    let mut expected_ids: Vec<String> = evil_attacks(shared, &companies, round).await;
    let scenario_ms = ms_since(produce_start);

    // Disseminate this round's private payloads concurrently: dissemination
    // is the dominant produce cost and is independent per payload.
    let payload_start = Instant::now();
    let accepted = disseminate_payloads(leader, std::mem::take(&mut round_payloads)).await;
    let payload_ms = ms_since(payload_start);
    let mut pricing: Vec<(String, String)> = Vec::new();
    {
        let mut ledger = shared.payloads.lock().await;
        for (kind, commitment, cleartext, author) in accepted {
            if kind == "pricing" {
                pricing.push((commitment.clone(), author.clone()));
            }
            ledger.push((COLLECTION.into(), commitment, cleartext, author));
        }
    }
    if !pricing.is_empty() {
        let mut state = shared.state.lock().await;
        for (commitment, author) in pricing {
            if state.pdc.iter().all(|entry| entry.collection != COLLECTION) {
                state.pdc.push(PdcEntry {
                    collection: COLLECTION.into(),
                    commitments: Vec::new(),
                });
            }
            if let Some(entry) = state
                .pdc
                .iter_mut()
                .find(|entry| entry.collection == COLLECTION)
            {
                entry.commitments.push(commitment);
            }
            // The dissemination hop, drawn so the private path is visible
            // even though its cleartext is not.
            for member in pricing_member_ids(&companies) {
                if member != author {
                    state.count_edge(&author, &member);
                }
            }
        }
    }
    // ── 5. Relay + mine: wait until every submission of this round reached
    // the star center's pool (contract-generated PurchaseOrders may trail
    // and land in the next block — admission vs commit stays honest).
    let batch_ids: Vec<String> = round_txs.iter().map(|(_, tx)| tx.id.clone()).collect();
    let submit_start = Instant::now();
    submit_round(shared, std::mem::take(&mut round_txs)).await;
    let submit_ms = ms_since(submit_start);
    expected_ids.extend(batch_ids);
    let settle_start = Instant::now();
    wait_for_ids(leader, &expected_ids).await;
    let settle_ms = ms_since(settle_start);
    let mine_start = Instant::now();
    let _ = leader.mine().await;
    let commit_ms = ms_since(mine_start);
    let project_start = Instant::now();

    // ── 6. Honest receipt: contract-generated PurchaseOrders that landed
    // in the committed block.
    {
        let ledger = leader.shared_ledger();
        let ledger = ledger.lock().await;
        let last = ledger.chain.last().cloned();
        drop(ledger);
        if let Some(block) = last {
            let mut state = shared.state.lock().await;
            for tx in &block.transactions {
                if let TransactionKind::PurchaseOrder(ref order) = tx.kind {
                    // The manual-completion row is pushed at submission;
                    // the committed block's scan updates the note only if
                    // the order has not already been recorded.
                    let already_recorded = state
                        .offers
                        .iter()
                        .any(|event| event.kind == "purchase" && event.tx_id == tx.id);
                    let manual = order
                        .contract_id
                        .as_deref()
                        .is_some_and(|contract| contract != CONTRACT_ID);
                    state.cash_move(
                        &order.buyer_id,
                        &order.seller_id,
                        order.quantity.saturating_mul(order.agreed_price_per_unit),
                    );
                    // The auto purchase fills the matching open offer from
                    // the same seller at the same price; partial lot buys
                    // show the fill like the manual ones do.
                    if !manual {
                        if let Some(row) = state.offers.iter_mut().find(|event| {
                            event.kind == "offer"
                                && event.seller == order.seller_id
                                && event.price_per_unit == order.agreed_price_per_unit
                                && event.sold < event.quantity
                        }) {
                            row.sold += order.quantity;
                            row.note = if row.sold >= row.quantity {
                                format!(
                                    "auto-executed: {0} of the lot's 500 units bought",
                                    row.sold
                                )
                            } else {
                                format!(
                                    "auto-executed: {0} of the lot's 500 units bought, \\
                                     offer re-advertised",
                                    row.sold
                                )
                            };
                        }
                    }
                    if already_recorded {
                        continue;
                    }
                    state.push_offer(OfferEvent {
                        kind: "purchase".into(),
                        tx_id: tx.id.clone(),
                        seller: order.seller_id.clone(),
                        buyer: order.buyer_id.clone(),
                        product: order.product_id.clone(),
                        quantity: order.quantity,
                        sold: 0,
                        price_per_unit: order.agreed_price_per_unit,
                        round,
                        note: if manual {
                            format!(
                                "manually completed by {} against {}, tx {}",
                                order.buyer_id,
                                order.contract_id.as_deref().unwrap_or(MANUAL_CONTRACT_ID),
                                tx.id
                            )
                        } else {
                            format!(
                                "matched by contract {CONTRACT_ID} (auto-execute), tx {}",
                                tx.id
                            )
                        },
                    });
                    state.record_tx(
                        tx.id.clone(),
                        "purchase",
                        &order.seller_id,
                        &order.buyer_id,
                        format!(
                            "PurchaseOrder {} × {} @ {} from {}",
                            order.quantity,
                            order.product_id,
                            order.agreed_price_per_unit,
                            order.seller_id
                        ),
                    );
                }
            }
        }
    }

    // ── 7. Chain receipt: custody stages, the tamper-evident block window,
    // the contract registry, the compliance projections and this round's
    // measured performance point.
    let (stage_by_seq, chains) = {
        let index = leader.provenance_index();
        let index = index.lock().await;
        let pipeline = shared.pipeline.lock().await;
        let stage_by_seq: Vec<(u64, u64)> = pipeline
            .iter()
            .map(|entry| (entry.seq, entry.stage))
            .collect();
        let chains: Vec<(u64, Vec<glasschain_indexer::provenance::CustodyEvent>)> = pipeline
            .iter()
            .map(|entry| {
                (
                    entry.seq,
                    index
                        .get_custody_chain(&asset_id(entry.seq, false))
                        .to_vec(),
                )
            })
            .collect();
        (stage_by_seq, chains)
    };

    // Compliance projections, folded incrementally: only flat records from
    // blocks newer than the projection cursor are aggregated, so sustained
    // stress runs stay O(new records) per round instead of O(all history).
    let last_projected = shared.state.lock().await.last_projected_block;
    let (new_records, lineage_reports) = {
        let provenance = leader.provenance_index();
        let provenance = provenance.lock().await;
        let flattener = leader.analytical_flattener();
        let flattener = flattener.lock().await;
        let pipeline = shared.pipeline.lock().await;
        let new_records: Vec<glasschain_indexer::FlatAssetRecord> = flattener
            .records()
            .iter()
            .filter(|record| record.block_index > last_projected)
            .cloned()
            .collect();
        let mut lineage_reports = Vec::new();
        for entry in pipeline.iter() {
            let report = validate_asset(&asset(&entry.manufacturer, entry.seq, false));
            // Per-asset lineage: the provenance index verifies the mandatory
            // custody events appear in order for THIS asset id (a GTIN-wide
            // flat-record match cannot prove one lot's chain).
            let lineage = provenance.verify_lineage(
                &asset_id(entry.seq, false),
                &["manufacture", "dispatch", "receive"],
            );
            lineage_reports.push((
                entry.seq,
                lineage,
                report.is_compliant,
                report.critical_count() as u64,
                report.warning_count() as u64,
                u64::try_from(report.field_count_present).unwrap_or(0),
            ));
        }
        (new_records, lineage_reports)
    };

    let blocks: Vec<BlockView> = {
        let ledger = leader.shared_ledger();
        let ledger = ledger.lock().await;
        ledger
            .chain
            .iter()
            .rev()
            .take(12)
            .map(|block| BlockView {
                height: block.index,
                hash: block.hash.chars().take(16).collect(),
                previous_hash: block.previous_hash.chars().take(16).collect(),
                tx_count: block.transactions.len() as u64,
                timestamp: block.timestamp,
                certified: block.certificate.is_some(),
            })
            .collect()
    };

    // Real contract registry, folded incrementally: definitions and committed
    // orders from blocks newer than the contract cursor, status and lifetime
    // purchases from the engine's summary.
    let last_contract_block = shared.state.lock().await.last_contract_block;
    let (new_contract_defs, new_orders, chain_tip) = {
        let ledger = leader.shared_ledger();
        let ledger = ledger.lock().await;
        let tip = ledger.chain.last().map_or(0, |block| block.index);
        let mut definitions = Vec::new();
        let mut executed: BTreeMap<String, u64> = BTreeMap::new();
        for block in ledger
            .chain
            .iter()
            .filter(|block| block.index > last_contract_block)
        {
            for tx in &block.transactions {
                match &tx.kind {
                    TransactionKind::ContractCreation(def) => definitions.push((
                        def.contract_id.clone(),
                        def.conditions.max_price_per_unit,
                        def.conditions.auto_execute,
                        def.conditions.max_quantity,
                        def.buyer_id.clone(),
                        def.product_id.clone(),
                    )),
                    TransactionKind::PurchaseOrder(order) => {
                        if let Some(contract_id) = &order.contract_id {
                            *executed.entry(contract_id.clone()).or_insert(0) += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        (definitions, executed, tip)
    };
    let contract_summaries = leader.contract_summaries().await;

    {
        let mut state = shared.state.lock().await;
        let stats = leader.pending_pool_stats().await;
        state.round = round;
        state.params = params.clone();
        state.chain_height = leader.shared_ledger().lock().await.chain.len() as u64;
        state.record_commit(
            commit_ms,
            u64::try_from(stats.count).unwrap_or(u64::MAX),
            u64::try_from(stats.bytes).unwrap_or(u64::MAX),
        );
        // One index build per round instead of a linear lot lookup per
        // pipeline entry — sustained runs must not be O(lots × pipeline).
        let lot_index: BTreeMap<u64, usize> = state
            .lots
            .iter()
            .enumerate()
            .filter_map(|(index, lot)| {
                lot.lot_ref
                    .strip_prefix("LOT-")
                    .and_then(|seq| seq.parse::<u64>().ok())
                    .map(|seq| (seq, index))
            })
            .collect();
        for (seq, events) in chains {
            let Some(&index) = lot_index.get(&seq) else {
                continue;
            };
            let view = &mut state.lots[index];
            view.chain = events
                .iter()
                .map(|event| LotStage {
                    event_type: event.event_type.clone(),
                    custodian: event.custodian_id.clone(),
                    block: event.block_index,
                })
                .collect();
            let stage = stage_by_seq
                .iter()
                .find(|(s, _)| *s == seq)
                .map_or(0, |(_, stage)| *stage);
            view.status = match stage {
                0 => "manufactured",
                1 => "at distributor",
                2 => "in transit",
                _ => "complete",
            }
            .into();
        }
        // Fold the new flat records into the cumulative compliance/trust
        // aggregates. The cursor only advances to the newest block actually
        // folded, so a late-ingested record is never missed.
        for record in &new_records {
            state.compliance.flat_records += 1;
            if record.is_standard_compliant {
                state.compliance.standard_records += 1;
            } else {
                state.compliance.low_trust_records += 1;
            }
            let trust = u64::from(record.trust_score);
            state.trust_total.0 += trust;
            state.trust_total.1 += 1;
            let org = state
                .trust_totals
                .entry(record.originator_id.clone())
                .or_default();
            org.0 += trust;
            org.1 += 1;
            if let Some(seq) = record
                .batch_number
                .as_deref()
                .and_then(|batch| batch.strip_prefix("B-"))
                .and_then(|digits| digits.parse::<u64>().ok())
            {
                let batch = state.batch_totals.entry(seq).or_default();
                batch.0 += trust;
                batch.1 += 1;
            }
            state.compliance.recent.insert(
                0,
                FlatRecordView {
                    block: record.block_index,
                    gtin: record.gtin.clone().unwrap_or_default(),
                    batch: record.batch_number.clone().unwrap_or_default(),
                    serial: record.serial_number.clone().unwrap_or_default(),
                    custodian: record.custodian_id.clone(),
                    event: record.event_type.clone(),
                    trust,
                    standard: record.is_standard_compliant,
                    missing: record.missing_core_fields.clone(),
                },
            );
        }
        state.compliance.recent.truncate(12);
        if let Some(newest) = new_records.iter().map(|record| record.block_index).max() {
            state.last_projected_block = state.last_projected_block.max(newest);
        }
        state.compliance.schema_version = format!("v{SCHEMA_VERSION_V1}");
        state.compliance.fields_total = 6;
        state.compliance.avg_trust = state
            .trust_total
            .0
            .checked_div(state.trust_total.1)
            .map_or(0, |avg| u8::try_from(avg).unwrap_or(u8::MAX));
        // Current-pipeline validation stats (recomputed every round).
        state.compliance.compliant = lineage_reports
            .iter()
            .filter(|(_, _, compliant, ..)| *compliant)
            .count() as u64;
        state.compliance.non_compliant = lineage_reports.len() as u64 - state.compliance.compliant;
        state.compliance.critical = lineage_reports
            .iter()
            .map(|(_, _, _, critical, ..)| critical)
            .sum();
        state.compliance.warnings = lineage_reports
            .iter()
            .map(|(_, _, _, _, warnings, _)| warnings)
            .sum();
        state.compliance.lineages_checked = lineage_reports.len() as u64;
        state.compliance.lineages_complete = lineage_reports
            .iter()
            .filter(|(_, lineage, ..)| *lineage)
            .count() as u64;
        for (seq, lineage, schema_compliant, ..) in &lineage_reports {
            let Some(&index) = lot_index.get(seq) else {
                continue;
            };
            let (sum, count) = state.batch_totals.get(seq).copied().unwrap_or((0, 0));
            let view = &mut state.lots[index];
            view.lineage_complete = *lineage;
            view.trust_avg = sum
                .checked_div(count)
                .map_or(0, |avg| u8::try_from(avg).unwrap_or(u8::MAX));
            view.flat_records = count;
            view.schema_compliant = *schema_compliant;
        }
        if let Some((_, _, _, _, _, fields)) = lineage_reports.last() {
            state.compliance.fields_present = *fields;
        }
        state.blocks = blocks;
        for (id, max_price, auto_execute, max_quantity, buyer, product) in new_contract_defs {
            if state.contracts.iter().all(|contract| contract.id != id) {
                state.contracts.push(ContractView {
                    executions: 0,
                    quantity_purchased: 0,
                    status: "Active".into(),
                    id,
                    buyer,
                    product,
                    max_price_per_unit: max_price,
                    max_quantity,
                    auto_execute,
                });
            }
        }
        for (id, count) in new_orders {
            if let Some(contract) = state
                .contracts
                .iter_mut()
                .find(|contract| contract.id == id)
            {
                contract.executions += count;
            }
        }
        for contract in &mut state.contracts {
            if let Some(summary) = contract_summaries
                .iter()
                .find(|summary| summary.id == contract.id)
            {
                contract.quantity_purchased = summary.quantity_purchased;
                contract.status = summary.status.clone();
            }
        }
        state.last_contract_block = chain_tip;
        // Per-org trust: the average MetadataTrustScore over the committed
        // registrations the org itself originated. Orgs with no registrations
        // stay at 0/0 — trust is undefined there, not assumed good.
        let totals = state.trust_totals.clone();
        for org in &mut state.orgs {
            match totals.get(&org.id) {
                Some((sum, count)) => {
                    org.trust_score = u8::try_from(sum / count).unwrap_or(u8::MAX);
                    org.records = *count;
                }
                None => {
                    org.trust_score = 0;
                    org.records = 0;
                }
            }
        }
        state.refresh_wms();
        state.refresh_throughput();
    }
    let project_ms = ms_since(project_start);

    // ── 8. Retail phase: pharmacies sell from their own inventories to end
    // customers, a little every round — stock accumulates first, and only
    // sustained rounds drain a warehouse completely. Each sale is a real
    // committed InventoryUpdate (quietly, off the live graph).
    let retail_start = Instant::now();
    for pharmacy in pharmacies {
        let sellable = {
            let state = shared.state.lock().await;
            state.sellable(pharmacy)
        };
        if sellable == 0 {
            continue;
        }
        // The drain scales with the warehouse: a fuller pharmacy sells
        // faster, so the loop can converge instead of accumulating forever.
        let pressure = {
            let state = shared.state.lock().await;
            buy_pressure(&state)
        };
        let units = sellable.min(RETAIL_UNITS_PER_ROUND * pressure);
        let sale = retail_sale_tx(pharmacy, units, round);
        let node = node_for(shared, pharmacy).await;
        if node.submit_transaction(sale).await.is_ok() {
            let mut state = shared.state.lock().await;
            state.metrics.submitted += 1;
            state.retail_sale(
                pharmacy,
                "pharmacy",
                units,
                units.saturating_mul(RETAIL_PRICE),
            );
            state.push_feed(format!(
                "{pharmacy} sold {units} units to customers (retail, committed next block)"
            ));
        }
    }
    let retail_ms = ms_since(retail_start);
    let round_ms = ms_since(round_start);
    {
        // The measured round point: throughput, commit latency and where the
        // wall-clock actually went. These are runner measurements, never
        // animation timestamps.
        let mut state = shared.state.lock().await;
        let point = RoundPoint {
            round,
            submitted: state.metrics.submitted,
            rejected: state.metrics.rejected,
            commit_ms,
            pool_count: state.metrics.pool_count,
            tx_per_sec: state.metrics.tx_per_sec,
            produce_ms: scenario_ms,
            payload_ms,
            submit_ms,
            settle_ms,
            project_ms,
            retail_ms,
            round_ms,
        };
        state.history.push(point);
        if state.history.len() > 64 {
            state.history.remove(0);
        }
    }
    log::info!(
        "round {round}: round {round_ms} ms = scenario {scenario_ms} + payload {payload_ms} + submit {submit_ms} + settle {settle_ms} + mine {commit_ms} + project {project_ms} + retail {retail_ms}"
    );
}

async fn evil_attacks(shared: &SharedRun, companies: &[Company], round: u64) -> Vec<String> {
    let seq = round * 100;
    let mut admitted: Vec<String> = Vec::new();
    for company in companies.iter().filter(|company| company.evil) {
        let node = node_for(shared, &company.id).await;
        // 1. private payload attempt — the collection gate or the fail-closed
        // verifier gate answers.
        let outcome = match node
            .submit_private_payload(COLLECTION, b"smuggled pricing data".to_vec())
            .await
        {
            Ok(()) => "ACCEPTED (unexpected!)".to_owned(),
            Err(_) => {
                if company.has_verifier {
                    "rejected by membership gate".to_owned()
                } else {
                    "rejected: fail-closed without a certificate verifier (#86)".to_owned()
                }
            }
        };
        {
            let mut state = shared.state.lock().await;
            state.push_security(SecurityEvent {
                actor: company.id.clone(),
                action: "private payload to pricing (not a member)".into(),
                outcome,
                detail: if company.has_verifier {
                    "collection membership enforced at the node".into()
                } else {
                    "org-gated paths fail closed".into()
                },
                explanation: if company.has_verifier {
                    "Private payloads only ever leave a node for peers whose organization                      is in the collection's member list. This company is verified but is                      not a member of `pricing`, so the membership gate refused before any                      cleartext could leave the node or reach a peer."
                        .to_owned()
                } else {
                    "Zero-trust gate (#86): private payloads require a configured,                      verified certificate chain. This node presents no verifier, so                      every org-gated path fails closed — the payload never left the                      node and nothing about it is in the chain."
                        .to_owned()
                },
            });
            state.metrics.rejected += 1;
        }

        // 2. forged canonical record with an undisclosed payload key — the
        // strict ADR-006 validator rejects anything the family descriptor
        // does not allow.
        let mut forged = cert_record(
            seq,
            "quality_certification",
            format!("forged-{seq}"),
            &company.id,
        );
        forged.payload.insert(
            "undisclosed_kickback".to_owned(),
            Value::String("rampant".into()),
        );
        let outcome = match node.submit_transaction(signed(forged, &company.id)).await {
            Ok(()) => "ACCEPTED (unexpected!)".to_owned(),
            Err(_) => "rejected by schema validation".to_owned(),
        };
        {
            let mut state = shared.state.lock().await;
            state.push_security(SecurityEvent {
                actor: company.id.clone(),
                action: "forged certification with undisclosed payload key".into(),
                outcome,
                detail: "strict canonical schema validation".into(),
                explanation: "ADR-006 records are strictly validated against the v1                      family descriptor: the `quality_certification` family allows only                      its listed payload keys. The forged record carried an extra field,                      so admission refused the whole transaction — nothing of it ever                      reached the pending pool, let alone a block."
                    .to_owned(),
            });
            state.metrics.rejected += 1;
        }

        // 3. tampered commitment: the lot's anchor references a commitment
        // the record does not hash to — anchored-family validation refuses.
        let mut tampered = lot_record(seq, &company.id);
        tampered.commitment = Some("deadbeef".repeat(8));
        let outcome = match node.submit_transaction(signed(tampered, &company.id)).await {
            Ok(()) => "ACCEPTED (unexpected!)".to_owned(),
            Err(_) => "rejected: commitment does not match the record".to_owned(),
        };
        {
            let mut state = shared.state.lock().await;
            state.push_security(SecurityEvent {
                actor: company.id.clone(),
                action: "lot anchor with tampered commitment".into(),
                outcome,
                detail: "anchored families hash-check the commitment".into(),
                explanation: "Anchored families (ADR-006) must carry commitment =                      sha256(canonical form). The forged anchor claimed a commitment its                      content does not produce, so strict validation refused it before                      admission."
                    .to_owned(),
            });
            state.metrics.rejected += 1;
        }

        // 4. duplicate replay: resubmit an already-pending transaction — the
        // idempotency gate must refuse the second copy.
        let replay = Transaction::with_id(
            format!("replay-{}", company.id),
            TransactionKind::CanonicalRecord(lot_record(seq, &company.id)),
        );
        let replay_node = node_for(shared, &company.id).await;
        let _ = replay_node.submit_transaction(replay.clone()).await;
        let outcome = match replay_node.submit_transaction(replay).await {
            Ok(()) => "ACCEPTED (unexpected!)".to_owned(),
            Err(_) => "rejected: duplicate transaction id (replay blocked)".to_owned(),
        };
        {
            let mut state = shared.state.lock().await;
            state.push_security(SecurityEvent {
                actor: company.id.clone(),
                action: "transaction replay (same id twice)".into(),
                outcome,
                detail: "pool idempotency".into(),
                explanation: "The pending pool and the committed index are idempotent by                      transaction id: a replayed transaction — even one the node itself                      just admitted — is refused, so an adversary cannot double-spend or                      double-count by replaying."
                    .to_owned(),
            });
            state.metrics.rejected += 1;
        }

        // 5. under-metadata registration: admitted (zero trust is not
        // exclusion) but scored and flagged.
        let evil_tx = custody_tx(seq, "manufacture", &company.id, true);
        let outcome = match node.submit_transaction(evil_tx.clone()).await {
            Ok(()) => {
                admitted.push(evil_tx.id);
                "admitted — non-standard metadata, trust score reduced".to_owned()
            }
            Err(_) => "rejected by admission".to_owned(),
        };
        {
            let mut state = shared.state.lock().await;
            state.push_security(SecurityEvent {
                actor: company.id.clone(),
                action: "registration with missing core metadata".into(),
                outcome,
                detail: "trust scoring flags the shortfall".into(),
                explanation: "Zero trust is not exclusion: the schema permits partial \
                     metadata, so the registration committed. But the chain scored it — \
                     MetadataTrustScore recorded the missing core fields (serial number, \
                     Anvisa registration, manufacturer id), dropping the score below the \
                     standard threshold. The shortfall is visible and permanent; the \
                     on-chain record devalues itself."
                    .to_owned(),
            });
        }
    }
    admitted
}

// ── Per-org views (read from that member's own node) ────────────────────────

/// What one member genuinely sees: its own node's chain height, its own
/// provenance index, and its own transient store — plus, for payloads the
/// runner disseminated, whether the cleartext actually landed in this
/// member's node. Not the leader's view laundered through a filter.
/// `org_snapshot` for `org` as seen through the VIEWER's rights: the page's
/// member selector sets the viewer, so opening another member's drawer
/// shows what the *viewer* can see — never the clicked member's private
/// view. `viewer == None` is the public lens: commitments only.
pub async fn org_snapshot(shared: &SharedRun, org: &str, viewer: Option<&str>) -> Value {
    if org == "admin" {
        // Admin (demo): the operator's eye — every payload's cleartext and
        // every company's cash balance. The chain itself carries no such
        // global view; this is the demo's privileged lens, clearly labelled.
        let payloads = shared.payloads.lock().await.clone();
        let state = shared.state.lock().await;
        let pdc_values: Vec<OrgPayload> = payloads
            .iter()
            .map(|(collection, commitment, cleartext, author)| {
                let lot = serde_json::from_str::<Value>(cleartext)
                    .ok()
                    .and_then(|terms| terms.get("lot").and_then(|lot| lot.as_u64()))
                    .unwrap_or(0);
                OrgPayload {
                    collection: collection.clone(),
                    commitment: commitment.clone(),
                    payload: Some(cleartext.clone()),
                    author: author.clone(),
                    lot,
                    kind: serde_json::from_str::<Value>(cleartext)
                        .ok()
                        .and_then(|terms| {
                            terms
                                .get("kind")
                                .and_then(|kind| kind.as_str())
                                .map(|kind| kind.to_owned())
                        })
                        .unwrap_or_else(|| "terms".to_owned()),
                    summary: serde_json::from_str::<Value>(cleartext)
                        .ok()
                        .map(|terms| payload_summary(&terms))
                        .unwrap_or_else(|| "—".to_owned()),
                }
            })
            .collect();
        return serde_json::json!({
            "org": "admin",
            "chain_height": state.chain_height,
            "pdc_values": pdc_values,
            "cash_all": state.cash,
            "private_visible": true,
            "pending_inbound": 0,
            "note": "Admin (demo) view — all payload cleartext and all cash balances are visible. The chain itself carries no global read; this lens exists for the demonstration only.",
        });
    }
    let node = shared.nodes.lock().await.get(org).cloned();
    let Some(node) = node else {
        return serde_json::json!({ "error": "unknown viewing org" });
    };
    let chain_height = node.shared_ledger().lock().await.chain.len() as u64;

    // Origin-scoped PDC view: the member reads ITS OWN cleartext (its node
    // holds it); other members' terms stay commitments; the regulator reads
    // everything. Cleartext presence is double-checked against the member's
    // own transient store when the payload is one it authored or the
    // regulator's global view.
    //
    // What gets LISTED is what the org owns: a member view shows only the
    // payloads that org authored. Readability is a separate, server-side
    // decision per payload (the author, the regulator and the admin lens
    // read; every other viewer gets the commitment). Clone only the listed
    // rows — cloning the whole ledger per request made some member
    // inspections slow.
    let listed: Vec<(String, String, String, String)> = {
        let payloads = shared.payloads.lock().await;
        match viewer {
            Some(v) if v.starts_with("regulator") => payloads.clone(),
            _ => payloads
                .iter()
                .filter(|(_, _, _, author)| author == org)
                .cloned()
                .collect(),
        }
    };
    let mut pdc_values = Vec::new();
    for (collection, commitment, cleartext, author) in listed {
        let terms_json = serde_json::from_str::<Value>(&cleartext).ok();
        let lot_number = terms_json
            .as_ref()
            .and_then(|terms| terms.get("lot").and_then(|lot| lot.as_u64()))
            .unwrap_or(0);
        // Readability is scoped server-side: the author reads its own
        // cleartext (checked against the node's own transient store), the
        // regulator and the admin lens read the collection, and every other
        // viewer gets the commitment only. UI hiding is not enforcement —
        // this filter is.
        let payload = match viewer {
            None => None,
            Some("admin") => Some(cleartext.clone()),
            Some(v) if v.starts_with("regulator") => Some(cleartext.clone()),
            Some(v) if author == v => {
                if node
                    .transient_payload(&collection, &commitment)
                    .await
                    .is_some()
                {
                    Some(cleartext.clone())
                } else {
                    None
                }
            }
            Some(_) => None,
        };
        pdc_values.push(OrgPayload {
            collection,
            commitment,
            payload,
            author,
            lot: lot_number,
            kind: terms_json
                .as_ref()
                .and_then(|terms| terms.get("kind").and_then(|kind| kind.as_str()))
                .unwrap_or("terms")
                .to_owned(),
            summary: terms_json
                .as_ref()
                .map(payload_summary)
                .unwrap_or_else(|| "—".to_owned()),
        });
    }
    // What this member holds in stock (committed custody), its demo cash
    // balance, and the offers where it is a counterparty.
    let state = shared.state.lock().await;
    let stock = state.wms.iter().find(|row| row.company == org);
    // Cash and trade bookkeeping are the member's private ledger: visible
    // to the member itself, the regulator, and Admin (demo) — not to the
    // public lens or to other members.
    let private_visible = viewer.is_some_and(|viewer| {
        viewer == org || viewer.starts_with("regulator") || viewer == "admin"
    });
    // Deliveries in flight addressed to this member (pipeline lots that
    // have not reached it yet).
    let pending_inbound = shared
        .pipeline
        .lock()
        .await
        .iter()
        .filter(|entry| entry.pharmacy == org && entry.stage < 3)
        .count() as u64;
    let cash = state.cash.get(org).copied().unwrap_or(CASH_START);
    let inventory = state.sellable(org);
    let sell_offers: Vec<&OfferEvent> = if private_visible {
        state
            .offers
            .iter()
            .filter(|event| event.kind == "offer" && event.seller == org)
            .collect()
    } else {
        Vec::new()
    };
    let purchases_as_buyer: Vec<&OfferEvent> = if private_visible {
        state
            .offers
            .iter()
            .filter(|event| event.kind == "purchase" && event.buyer == org)
            .collect()
    } else {
        Vec::new()
    };
    let sells: Vec<serde_json::Value> = sell_offers
        .iter()
        .map(|offer| {
            serde_json::json!({
                "tx_id": offer.tx_id,
                "product": offer.product,
                "quantity": offer.quantity,
                "sold": offer.sold,
                "price_per_unit": offer.price_per_unit,
                "round": offer.round,
                "buyer": offer.buyer,
                "note": offer.note,
            })
        })
        .collect();
    let buys: Vec<serde_json::Value> = purchases_as_buyer
        .iter()
        .map(|order| {
            serde_json::json!({
                "tx_id": order.tx_id,
                "product": order.product,
                "quantity": order.quantity,
                "price_per_unit": order.price_per_unit,
                "round": order.round,
                "seller": order.seller,
            })
        })
        .collect();
    serde_json::json!({
        "org": org,
        "chain_height": chain_height,
        "pdc_values": pdc_values,
        "stock": stock.map(|row| serde_json::json!({
            "lots_here": row.lots_here,
            "units_here": row.units_here,
            "received_units": row.received_units,
            "dispatched_units": row.dispatched_units,
            "sold_units": row.sold_units,
        })),
        "inventory": inventory,
        "cash": if private_visible { serde_json::json!(cash) } else { serde_json::Value::Null },
        "sell_offers": if private_visible { sells } else { Vec::new() },
        "purchases": if private_visible { buys } else { Vec::new() },
        "pending_inbound": pending_inbound,
        "private_visible": private_visible,
    })
}

// ── The runner ──────────────────────────────────────────────────────────────

/// The long-running demo task. The caller stores the returned join handle so
/// the bridge can stop it with an abort. Parameter changes apply live:
/// topology-affecting changes rebuild the federation at a round boundary.
pub fn spawn_run(shared: Arc<SharedRun>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut topology_version = shared
            .topology_version
            .load(std::sync::atomic::Ordering::SeqCst);
        let mut leader = setup_run(&shared).await;

        // Surface equivocation evidence if the staged BFT engine ever emits
        // it (labeled honestly — the dev driver is PoW). Also surface
        // contract executions so the UI shows the match flow.
        let watcher_shared = Arc::clone(&shared);
        let mut events = leader.subscribe();
        let _watcher = tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(NodeEvent::EquivocationDetected { height, .. }) => {
                        watcher_shared
                            .state
                            .lock()
                            .await
                            .equivocations
                            .push(format!("equivocation evidence at height {height}"));
                    }
                    Ok(NodeEvent::ContractExecuted {
                        contract_id,
                        quantity,
                    }) => {
                        let mut state = watcher_shared.state.lock().await;
                        let round = state.round;
                        state.push_offer(OfferEvent {
                            kind: "execution".into(),
                            tx_id: String::new(),
                            seller: "—".into(),
                            buyer: "—".into(),
                            product: "—".into(),
                            quantity,
                            sold: 0,
                            price_per_unit: 0,
                            round,
                            note: format!("contract {contract_id} executed"),
                        });
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        log::warn!("demo event lag: {count}");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        loop {
            let current_version = shared
                .topology_version
                .load(std::sync::atomic::Ordering::SeqCst);
            if current_version != topology_version {
                log::info!("topology changed: rebuilding the federation");
                {
                    let mut state = shared.state.lock().await;
                    state.status = "rebuilding".into();
                }
                leader = setup_run(&shared).await;
                topology_version = current_version;
            }
            let interval = params_snapshot(&shared).await.round_interval_ms;
            let round = shared.state.lock().await.round + 1;
            drive_round(&shared, &leader, round).await;
            tokio::time::sleep(Duration::from_millis(interval)).await;
        }
    })
}

/// Test seam: one full synchronized round against a fresh federation.
#[cfg(test)]
pub async fn run_one_round(shared: &SharedRun) {
    run_rounds(shared, 1).await;
}

/// Test seam: `rounds` synchronized rounds (the pipeline advances one hop
/// per round, so four rounds move a lot to the pharmacy).
#[cfg(test)]
pub async fn run_rounds(shared: &SharedRun, rounds: u64) {
    let leader = setup_run(shared).await;
    for round in 1..=rounds {
        drive_round(shared, &leader, round).await;
    }
    drop(leader);
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn one_round_drives_the_whole_scenario() {
        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 5).await;
        let state = shared.state.lock().await;
        assert_eq!(state.status, "running");
        assert_eq!(state.orgs.len(), build_companies(&state.params).len());
        assert!(state.metrics.blocks >= 1, "the round mined a block");
        let complete = state.lots.iter().any(|lot| lot.status == "complete");
        assert!(complete, "a lot reached the pharmacy through the chain");
        assert_eq!(state.pdc.len(), 1);
        let payloads = shared.payloads.lock().await;
        assert!(payloads.iter().any(|(collection, _, payload, _)| {
            collection == COLLECTION && payload.contains("\"kind\":\"pricing\"")
        }));
        assert!(payloads.iter().any(|(collection, _, payload, _)| {
            collection == COLLECTION && payload.contains("\"kind\":\"storage\"")
        }));
        assert!(payloads.iter().any(|(collection, _, payload, _)| {
            collection == COLLECTION && payload.contains("\"kind\":\"transit\"")
        }));
        assert!(payloads.iter().any(|(collection, _, payload, _)| {
            collection == COLLECTION && payload.contains("\"kind\":\"certification_evidence\"")
        }));
        assert!(payloads.iter().any(|(collection, _, payload, _)| {
            collection == COLLECTION && payload.contains("\"kind\":\"regulator_notes\"")
        }));
        assert!(state.metrics.submitted > 0);
        assert!(!state.offers.is_empty(), "the contract flow is visible");
        assert!(state
            .certs
            .iter()
            .any(|cert| cert.schema == "audit_attestation"));
        assert!(!state.edges.is_empty(), "edge counters exist");
        assert!(!state.wms.is_empty(), "WMS rows exist");
        let manufacturer_row = state
            .wms
            .iter()
            .find(|row| row.company == "manufacturer-1")
            .expect("manufacturer-1 WMS row");
        assert!(manufacturer_row.received_units > 0);
        assert!(manufacturer_row.dispatched_units > 0);
        let pharmacy_row = state
            .wms
            .iter()
            .find(|row| row.company == "pharmacy-1")
            .expect("pharmacy-1 WMS row");
        eprintln!(
            "debug: lot1chain={:?} block_tails={:?} wms={:?}",
            state
                .lots
                .iter()
                .map(|lot| (
                    lot.lot_ref.clone(),
                    lot.chain
                        .iter()
                        .map(|stage| format!("{}:{}", stage.event_type, stage.custodian))
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            state.lots.len(),
            state
                .wms
                .iter()
                .map(|row| (row.company.clone(), row.lots_here))
                .collect::<Vec<_>>()
        );
        assert!(pharmacy_row.lots_here >= 1, "a lot sits at the pharmacy");
    }

    #[tokio::test]
    async fn wms_summary_values_the_fleet() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let state = shared.state.lock().await;
        let summary = &state.wms_summary;
        assert_eq!(summary.members, state.wms.len() as u64);
        assert_eq!(
            summary.total_units,
            state.wms.iter().map(|r| r.units_here).sum::<u64>()
        );
        assert_eq!(
            summary.total_value_minor,
            state.wms.iter().map(|r| r.stock_value_minor).sum::<u64>()
        );
        assert_eq!(
            summary.total_value_minor,
            summary.total_units * RETAIL_PRICE
        );
        assert!(summary.low_stock <= summary.members);
        let stocked = state
            .wms
            .iter()
            .find(|row| row.units_here > 0)
            .expect("a stocked member");
        assert!(stocked.stock_value_minor > 0);
    }

    #[tokio::test]
    async fn retail_sales_drain_inventory_slowly() {
        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 6).await;
        let state = shared.state.lock().await;
        // Pharmacies received stock this round; the retail phase sold at
        // most RETAIL_UNITS_PER_ROUND from any of them — nobody dumps their
        // whole inventory at once.
        let pharmacy = "pharmacy-1";
        let row = state
            .wms
            .iter()
            .find(|row| row.company == pharmacy)
            .expect("pharmacy WMS row");
        let sold = row.sold_units;
        // Six rounds, at most RETAIL × PRESSURE units each — slow and
        // bounded, never a sell-through dump.
        assert!(
            sold <= RETAIL_UNITS_PER_ROUND * PRESSURE_CAP * 6,
            "retail drains slowly and boundedly: {sold}"
        );
        assert!(sold > 0, "a stocked pharmacy sold retail");
        if sold > 0 {
            let inventory = state.sellable(pharmacy);
            assert!(
                inventory > 0 || sold >= 500,
                "stock remains after a slow drain"
            );
            // The retail sale was registered as a real chain event.
            assert!(state.metrics.submitted > 0);
        }
    }

    #[tokio::test]
    async fn auto_purchases_fill_offers_and_show_partial_lots() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let state = shared.state.lock().await;
        // Offers ladder over lot sizes (300/400/500): the auto purchase
        // fills the offer and the note reports the partial lot buy.
        let filled = state
            .offers
            .iter()
            .find(|event| event.kind == "offer" && event.note.contains("auto-executed"));
        assert!(
            filled.is_some(),
            "the auto purchase marked its offer as filled: {:?}",
            state
                .offers
                .iter()
                .map(|o| (o.kind.clone(), o.note.clone()))
                .collect::<Vec<_>>()
        );
        let filled = filled.expect("checked");
        assert!(filled.sold > 0);
        assert!(filled.note.contains("of the lot's 500 units"));
        // The offer quantity ladder varies: not every auto buy is the same size.
        let quantities: std::collections::HashSet<u64> = state
            .offers
            .iter()
            .filter(|event| event.kind == "offer")
            .map(|event| event.quantity)
            .collect();
        assert!(quantities.len() > 1, "offers ladder over lot fractions");
    }

    #[tokio::test]
    async fn lots_table_keeps_history_without_downsizing() {
        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 3).await;
        let count = shared.state.lock().await.lots.len();
        // Three rounds × three lots: all nine still listed.
        assert_eq!(count, 9, "lots are append-only, never shed mid-session");
    }

    #[tokio::test]
    async fn manual_purchase_completes_a_pending_offer() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let pending = {
            let state = shared.state.lock().await;
            state
                .offers
                .iter()
                .find(|event| event.kind == "offer" && event.note.contains("awaiting"))
                .map(|event| (event.tx_id.clone(), event.seller.clone()))
                .expect("a premium offer awaited a buyer")
        };
        let (tx_id, seller) = pending;
        let submitted = complete_purchase(&shared, &tx_id, "pharmacy-1", 500).await;
        assert!(submitted.is_ok(), "the manual purchase passed admission");
        // The next round's mine commits the manually-raised order.
        let leader = node_for(&shared, "manufacturer-1").await;
        let _ = leader.mine().await;
        let state = shared.state.lock().await;
        let manual = state
            .offers
            .iter()
            .find(|event| event.kind == "purchase" && event.note.contains("manually completed"));
        assert!(manual.is_some(), "the manual purchase is on the ledger");
        let manual = manual.expect("checked");
        assert_eq!(manual.seller, seller);
        assert_eq!(manual.buyer, "pharmacy-1");
    }

    #[tokio::test]
    async fn partial_manual_buys_leave_the_offer_open() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let (pending, price, quantity, cash_before_1, cash_before_2) = {
            let state = shared.state.lock().await;
            let row = state
                .offers
                .iter()
                .find(|event| event.kind == "offer" && event.note.contains("awaiting"))
                .cloned()
                .expect("a premium offer awaited a buyer");
            (
                row.tx_id.clone(),
                row.price_per_unit,
                row.quantity,
                state.cash.get("pharmacy-1").copied().unwrap_or(CASH_START),
                state.cash.get("pharmacy-2").copied().unwrap_or(CASH_START),
            )
        };
        assert!(complete_purchase(&shared, &pending, "pharmacy-1", 100)
            .await
            .is_ok());
        assert!(
            complete_purchase(&shared, &pending, "pharmacy-2", 100)
                .await
                .is_ok(),
            "the offer stays open after a partial buy"
        );
        let (sold, note) = {
            let state = shared.state.lock().await;
            let row = state
                .offers
                .iter()
                .find(|event| event.kind == "offer" && event.tx_id == pending)
                .cloned()
                .expect("the offer row");
            (row.sold, row.note.clone())
        };
        assert_eq!(sold, 200);
        assert!(
            note.contains("still on offer"),
            "the offer stays open: {note}"
        );
        // Cash deltas: each buyer paid its 100 units (demo bookkeeping).
        assert_eq!(
            shared
                .state
                .lock()
                .await
                .cash
                .get("pharmacy-1")
                .copied()
                .unwrap_or(CASH_START),
            cash_before_1 - 100 * price
        );
        assert_eq!(
            shared
                .state
                .lock()
                .await
                .cash
                .get("pharmacy-2")
                .copied()
                .unwrap_or(CASH_START),
            cash_before_2 - 100 * price
        );
        // Buying the remainder closes the offer with a sold-out note.
        assert!(complete_purchase(&shared, &pending, "pharmacy-1", 500)
            .await
            .is_ok());
        let (sold, note) = {
            let state = shared.state.lock().await;
            let row = state
                .offers
                .iter()
                .find(|event| event.kind == "offer" && event.tx_id == pending)
                .cloned()
                .expect("the offer row");
            (row.sold, row.note.clone())
        };
        // The oversized buy clamps to the offer's remainder, and the offer
        // closes with the sold-out note.
        assert_eq!(sold, quantity);
        assert!(note.contains("fully bought"));
    }

    #[tokio::test]
    async fn member_views_are_origin_scoped() {
        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 5).await;
        // manufacturer-1 reads ITS OWN terms in cleartext…
        let own = org_snapshot(&shared, "manufacturer-1", Some("manufacturer-1")).await;
        let own_readable = own["pdc_values"]
            .as_array()
            .expect("array")
            .iter()
            .filter(|entry| entry["payload"].is_string())
            .count();
        assert!(own_readable > 0, "a member reads its own terms");
        // The list contains ONLY the member's own payloads — the other
        // members' payloads were never in its view.
        let view = own["pdc_values"].as_array().expect("array");
        assert!(
            view.iter().all(|entry| entry["author"] == "manufacturer-1"),
            "manufacturer-1's view lists only its own payloads"
        );
        assert!(
            view.iter().all(|entry| entry["payload"].is_string()),
            "every listed payload is readable — ownership scopes the list"
        );
        assert!(view.iter().any(|entry| {
            entry["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("\"kind\":\"pricing\""))
        }));
        // The other kinds (storage/transit) are in the list as COMMITMENTS —
        // kind data is only visible in cleartext, so the null rows authored
        // by the distributor/logistics carry them implicitly.
        assert!(view
            .iter()
            .filter(|entry| entry["payload"].is_string())
            .all(|entry| entry["author"] == "manufacturer-1"));
        // ...but only the VIEWER's own payloads are readable; the rest of
        // the kinds appear as commitments to manufacturer-1.
        assert!(view
            .iter()
            .filter(|entry| entry["payload"].is_string())
            .all(|entry| entry["author"] == "manufacturer-1"));
        // …inspecting ANOTHER member through a member lens stays
        // commitments-only: origin scoping is enforced server-side.
        let cross = org_snapshot(&shared, "manufacturer-2", Some("manufacturer-1")).await;
        assert!(
            cross["pdc_values"]
                .as_array()
                .expect("array")
                .iter()
                .all(|entry| entry["payload"].is_null()),
            "a member reads another member's payloads as commitments only"
        );
        // …the regulator reads everything…
        let regulator = org_snapshot(&shared, "regulator-1", Some("regulator-1")).await;
        assert!(
            regulator["pdc_values"]
                .as_array()
                .expect("array")
                .iter()
                .all(|entry| entry["payload"].is_string()),
            "the regulator holds every cleartext"
        );
        // …and the certifier still reads no cleartext: every entry is a
        // commitment-only row.
        let certifier = org_snapshot(&shared, "certifier-1", Some("certifier-1")).await;
        assert!(certifier["pdc_values"]
            .as_array()
            .expect("array")
            .iter()
            .all(|entry| entry["payload"].is_null()));
    }

    #[tokio::test]
    async fn member_views_show_stock_cash_and_offers() {
        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 5).await;
        let view = org_snapshot(&shared, "manufacturer-1", Some("manufacturer-1")).await;
        assert!(
            view["stock"].is_object(),
            "the member's stock is in the view"
        );
        assert!(view["cash"].as_u64().expect("cash") > 0);
        assert!(
            !view["sell_offers"].as_array().expect("array").is_empty(),
            "the manufacturer's advertised sell offers are listed"
        );
        // Private bookkeeping is exactly scoped: the member's own lens sees
        // it, another member's lens does not.
        assert_eq!(view["private_visible"], true);
        // The admin lens is private-visible too (its own `||` arm).
        let admin = org_snapshot(&shared, "manufacturer-1", Some("admin")).await;
        assert_eq!(admin["private_visible"], true);
        let other = org_snapshot(&shared, "manufacturer-1", Some("pharmacy-1")).await;
        assert_eq!(other["private_visible"], false);
        assert!(other["cash"].is_null());
        assert!(other["sell_offers"].as_array().expect("array").is_empty());

        // The listed sells/buys are exactly this org's offer/purchase events,
        // by transaction id — a count alone would not separate an offer from
        // its generated purchase.
        let state = shared.state.lock().await;
        let expected_sells: Vec<String> = state
            .offers
            .iter()
            .filter(|event| event.kind == "offer" && event.seller == "manufacturer-1")
            .map(|event| event.tx_id.clone())
            .collect();
        let expected_buys: Vec<String> = state
            .offers
            .iter()
            .filter(|event| event.kind == "purchase" && event.buyer == "pharmacy-1")
            .map(|event| event.tx_id.clone())
            .collect();
        drop(state);
        let actual_sells: Vec<String> = view["sell_offers"]
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|entry| entry["tx_id"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(actual_sells, expected_sells);

        // A buyer's view lists exactly its own purchases (the auto contract's
        // buyer is pharmacy-1, so the list is not empty).
        let pharmacy = org_snapshot(&shared, "pharmacy-1", Some("pharmacy-1")).await;
        let actual_buys: Vec<String> = pharmacy["purchases"]
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|entry| entry["tx_id"].as_str().map(str::to_owned))
            .collect();
        assert!(!expected_buys.is_empty());
        assert_eq!(actual_buys, expected_buys);
        assert!(pharmacy["stock"].is_object());
        // A member with no warehouse row has no stock: the lookup is by exact
        // company, never "some other row".
        let regulator = org_snapshot(&shared, "regulator-1", Some("regulator-1")).await;
        assert!(regulator["stock"].is_null());
        // A non-pharmacy never has inbound deliveries; a pharmacy's count is
        // the strict `stage < 3` slice of the pipeline.
        let manufacturer = org_snapshot(&shared, "manufacturer-1", Some("manufacturer-1")).await;
        assert_eq!(manufacturer["pending_inbound"].as_u64(), Some(0));
        let pipeline = shared.pipeline.lock().await;
        let total = pipeline
            .iter()
            .filter(|entry| entry.pharmacy == "pharmacy-1")
            .count();
        let delivered = pipeline
            .iter()
            .filter(|entry| entry.pharmacy == "pharmacy-1" && entry.stage >= 3)
            .count();
        drop(pipeline);
        assert_eq!(
            pharmacy["pending_inbound"].as_u64(),
            Some((total - delivered) as u64)
        );
    }

    #[tokio::test]
    async fn security_events_carry_explanations() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let state = shared.state.lock().await;
        assert!(state
            .security
            .iter()
            .all(|event| { !event.explanation.is_empty() && event.explanation.len() > 40 }));
        assert!(state
            .security
            .iter()
            .any(|event| event.explanation.contains("membership")));
        assert!(state
            .security
            .iter()
            .any(|event| event.explanation.contains("fails closed")));
    }

    #[tokio::test]
    async fn contract_engine_matches_offers_into_purchase_orders() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let state = shared.state.lock().await;
        let offers = state
            .offers
            .iter()
            .filter(|event| event.kind == "offer")
            .count();
        let purchases = state
            .offers
            .iter()
            .filter(|event| event.kind == "purchase")
            .count();
        assert!(offers >= 1, "at least one supply offer was submitted");
        assert!(
            purchases >= 1,
            "the contract engine auto-executed a purchase order"
        );
        let purchase = state
            .offers
            .iter()
            .find(|event| event.kind == "purchase")
            .expect("checked above");
        assert!(purchase.note.contains(CONTRACT_ID));
        assert_eq!(purchase.buyer, "pharmacy-1");
    }

    #[tokio::test]
    async fn evil_attacks_die_on_the_real_gates() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let state = shared.state.lock().await;
        let events = &state.security;
        // evil-1: verified but excluded — the membership gate answers.
        assert!(events
            .iter()
            .any(|event| event.actor == "evil-1" && event.outcome.contains("membership gate")));
        // evil-2: no verifier — the fail-closed gate answers.
        assert!(events
            .iter()
            .any(|event| event.actor == "evil-2" && event.outcome.contains("fail-closed")));
        // Nothing evil ever entered the pricing payloads: they are all
        // structured terms with the member price.
        let payloads = shared.payloads.lock().await;
        assert!(payloads
            .iter()
            .all(|(_, _, payload, _)| payload.contains("\"kind\"")));
    }

    #[tokio::test]
    async fn org_views_come_from_each_members_own_node() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let regulator = org_snapshot(&shared, "regulator-1", Some("regulator-1")).await;
        let certifier = org_snapshot(&shared, "certifier-1", Some("certifier-1")).await;
        let regulator_values = regulator["pdc_values"].as_array().expect("array");
        assert!(
            !regulator_values.is_empty(),
            "the regulator is a member and holds the payloads"
        );
        assert!(certifier["pdc_values"]
            .as_array()
            .expect("array")
            .iter()
            .all(|entry| entry["payload"].is_null()));
        assert_eq!(regulator["chain_height"], certifier["chain_height"]);

        // Membership is exact: honest non-certifier companies are in
        // `pricing`; certifiers and evil nodes are not.
        let state = shared.state.lock().await;
        assert!(state
            .orgs
            .iter()
            .filter(|org| org.evil || org.role == "certifier")
            .all(|org| org.member_of.is_empty()));
        assert!(state
            .orgs
            .iter()
            .filter(|org| !org.evil && org.role != "certifier")
            .all(|org| org.member_of == vec![COLLECTION.to_owned()]));
    }

    #[tokio::test]
    async fn snapshot_sells_security_compliance_and_performance() {
        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 4).await;
        let value = shared.snapshot_value().await;

        // Tamper-evident block window: every block carries hash links.
        let blocks = value["blocks"].as_array().expect("blocks");
        assert!(!blocks.is_empty(), "the chain window is exposed");
        assert!(blocks
            .iter()
            .all(|block| block["hash"].is_string() && block["previous_hash"].is_string()));

        // Real security posture: OCSP staples minted per member and verified
        // locally, verifier presence per node.
        let posture = value["posture"].as_array().expect("posture");
        assert_eq!(posture.len(), 15, "every company reports posture");
        assert!(
            posture.iter().any(|row| row["ocsp"]
                .as_str()
                .is_some_and(|ocsp| ocsp.contains("verified locally"))),
            "staples verify against the issuer"
        );
        assert!(posture.iter().any(|row| row["verifier"] == true));
        assert!(
            posture.iter().any(|row| row["verifier"] == false),
            "the verifier-less evil node is visible"
        );
        assert!(posture.iter().any(|row| row["collections"]
            .as_array()
            .is_some_and(|list| !list.is_empty())));

        // Real contracts with their conditions.
        let contracts = value["contracts"].as_array().expect("contracts");
        assert!(
            contracts
                .iter()
                .any(|contract| contract["id"] == "auto-replenish"
                    && contract["auto_execute"] == true)
        );
        assert!(
            contracts
                .iter()
                .any(|contract| contract["id"] == "manual-review"
                    && contract["auto_execute"] == false)
        );

        // Compliance rollup from the leader's projections, with per-lot
        // provenance lineage: after four rounds the first lots have their
        // full manufacture → dispatch → dispatch → receive sequence.
        let compliance = &value["compliance"];
        // `setup_run` seeds the schema identity, not an empty default.
        let expected_schema = format!("v{SCHEMA_VERSION_V1}");
        assert_eq!(
            compliance["schema_version"].as_str(),
            Some(expected_schema.as_str())
        );
        assert_eq!(compliance["fields_total"].as_u64(), Some(6));
        assert!(compliance["flat_records"].as_u64().unwrap_or(0) > 0);
        assert!(compliance["lineages_checked"].as_u64().unwrap_or(0) > 0);
        assert!(
            compliance["lineages_complete"].as_u64().unwrap_or(0) > 0,
            "a completed lot's mandatory custody events verify in order"
        );
        assert!(compliance["compliant"].as_u64().unwrap_or(0) > 0);
        assert!(!compliance["recent"].as_array().expect("recent").is_empty());

        // One measured performance point per round.
        let history = value["history"].as_array().expect("history");
        assert_eq!(history.len(), 4);
        assert!(history
            .iter()
            .all(|point| point["commit_ms"].as_u64().is_some()));
    }

    #[tokio::test]
    async fn offers_vary_and_orgs_carry_trust_scores() {
        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 5).await;
        let state = shared.state.lock().await;

        // Prices and quantities vary: not every offer is a full lot at
        // $10–$11, and every third offer is a premium on a manual band.
        let offers: Vec<&OfferEvent> = state
            .offers
            .iter()
            .filter(|event| event.kind == "offer")
            .collect();
        let prices: std::collections::HashSet<u64> =
            offers.iter().map(|offer| offer.price_per_unit).collect();
        let quantities: std::collections::HashSet<u64> =
            offers.iter().map(|offer| offer.quantity).collect();
        assert!(prices.len() >= 4, "prices vary: {prices:?}");
        assert!(quantities.len() >= 4, "quantities vary: {quantities:?}");
        let premium_buyers: std::collections::HashSet<&str> = offers
            .iter()
            .filter(|offer| offer.price_per_unit > CONTRACT_MAX_PRICE)
            .map(|offer| offer.buyer.as_str())
            .collect();
        assert!(
            premium_buyers.len() >= 2,
            "premium offers rotate across the manual contracts: {premium_buyers:?}"
        );

        // All four contracts are registered with their real conditions.
        let contracts: std::collections::HashSet<&str> =
            state.contracts.iter().map(|c| c.id.as_str()).collect();
        for id in [
            CONTRACT_ID,
            MANUAL_CONTRACT_ID,
            VALUE_CONTRACT_ID,
            PREMIUM_CONTRACT_ID,
        ] {
            assert!(contracts.contains(id), "contract {id} registered");
        }

        // Per-org trust from real registrations: honest makers score high,
        // the evil under-metadata node is flagged low, certifiers have none.
        let maker = state
            .orgs
            .iter()
            .find(|org| org.id == "manufacturer-1")
            .expect("maker org");
        assert!(
            maker.records > 0 && maker.trust_score >= 80,
            "honest maker trust: {maker:?}"
        );
        let evil = state
            .orgs
            .iter()
            .find(|org| org.id == "evil-1")
            .expect("evil org");
        assert!(
            evil.records > 0 && evil.trust_score < 80,
            "evil trust drop: {evil:?}"
        );
        let certifier = state
            .orgs
            .iter()
            .find(|org| org.role == "certifier" && !org.evil)
            .expect("certifier org");
        assert_eq!(
            certifier.records, 0,
            "certifiers originate no registrations"
        );
    }

    #[tokio::test]
    async fn round_points_carry_phase_breakdowns() {
        // Stress bounds move in lockstep with the form: 50 lots/round and a
        // zero interval (back-to-back rounds) are valid.
        let params = SimParams {
            lots_per_round: 200,
            round_interval_ms: 0,
            ..SimParams::defaults()
        }
        .sanitized();
        assert_eq!(
            params.lots_per_round, 50,
            "lots/round clamps to the stress bound"
        );
        assert_eq!(params.round_interval_ms, 0, "zero interval is allowed");

        let shared = Arc::new(SharedRun::default());
        run_rounds(&shared, 3).await;
        let value = shared.snapshot_value().await;
        let history = value["history"].as_array().expect("history");
        let point = history.last().expect("a round point");
        for key in [
            "produce_ms",
            "payload_ms",
            "submit_ms",
            "settle_ms",
            "commit_ms",
            "project_ms",
            "retail_ms",
            "round_ms",
        ] {
            assert!(point[key].as_u64().is_some(), "phase {key} is measured");
        }
        // The phases break the round down; they must not add up past it
        // (small scheduling overhead between timers is allowed).
        let phases: u64 = [
            "produce_ms",
            "payload_ms",
            "submit_ms",
            "settle_ms",
            "commit_ms",
            "project_ms",
            "retail_ms",
        ]
        .iter()
        .map(|key| point[key].as_u64().unwrap_or(0))
        .sum();
        let total = point["round_ms"].as_u64().unwrap_or(0);
        assert!(
            phases <= total + 50,
            "phases {phases} ms vs round {total} ms"
        );

        // Latency tails are measured, not inferred: commit p99 is reported
        // and sits at or above p95 for the same rolling window.
        let metrics = &value["metrics"];
        let p95 = metrics["commit_p95_ms"].as_u64().expect("commit p95");
        let p99 = metrics["commit_p99_ms"].as_u64().expect("commit p99");
        assert!(p99 >= p95, "p99 {p99} ms ≥ p95 {p95} ms");
    }

    #[test]
    fn percentile_is_exact_at_the_boundaries() {
        assert_eq!(percentile(&[], 50), 0);
        assert_eq!(percentile(&[5], 50), 5);
        assert_eq!(percentile(&[4, 1, 3, 2], 50), 3);
        assert_eq!(percentile(&[1, 2, 3, 4], 95), 4);
        assert_eq!(percentile(&[10, 20], 0), 10);
        assert_eq!(percentile(&[10, 20], 100), 20);
    }

    #[test]
    fn run_state_edges_feed_and_commit_metrics_are_exact() {
        let mut state = RunState::default();

        state.count_edge("a", "b");
        state.count_edge("a", "b");
        state.count_edge("b", "c");
        assert_eq!(state.edges.len(), 2);
        assert_eq!(state.edges[0].from, "a");
        assert_eq!(state.edges[0].count, 2);
        assert_eq!(state.edges[1].to, "c");
        assert_eq!(state.edges[1].count, 1);

        for index in 0..FEED_CAP + 3 {
            state.push_feed(format!("item-{index}"));
        }
        assert_eq!(state.feed.len(), FEED_CAP);
        assert_eq!(state.feed[0].label, "item-3");
        assert_eq!(state.feed[0].height, state.chain_height);

        for commit_ms in [100, 200, 300, 400] {
            state.record_commit(commit_ms, 7, 70);
        }
        assert_eq!(state.metrics.blocks, 4);
        assert_eq!(state.metrics.last_commit_ms, 400);
        assert_eq!(state.metrics.pool_count, 7);
        assert_eq!(state.metrics.pool_bytes, 70);
        assert_eq!(state.metrics.commit_p50_ms, 300);
        assert_eq!(state.metrics.commit_p95_ms, 400);
    }

    // ── Pure helpers and RunState bookkeeping ───────────────────────────────

    fn security_event(actor: &str) -> SecurityEvent {
        SecurityEvent {
            actor: actor.into(),
            action: "probe".into(),
            outcome: "rejected".into(),
            detail: "detail".into(),
            explanation: "explanation".into(),
        }
    }

    fn offer_event(tx_id: &str) -> OfferEvent {
        OfferEvent {
            kind: "offer".into(),
            tx_id: tx_id.into(),
            seller: "seller".into(),
            buyer: "buyer".into(),
            product: "SKU-DEMO".into(),
            quantity: 100,
            sold: 0,
            price_per_unit: 1_000,
            round: 1,
            note: "awaiting".into(),
        }
    }

    #[test]
    fn company_roles_and_evil_flavors_are_exact() {
        let params = SimParams {
            manufacturers: 2,
            distributors: 1,
            logistics: 1,
            pharmacies: 3,
            regulators: 1,
            certifiers: 1,
            evil_nodes: 3,
            lots_per_round: 1,
            round_interval_ms: 1,
        };
        let companies = build_companies(&params);
        let count = |role: &str| {
            companies
                .iter()
                .filter(|company| company.role == role && !company.evil)
                .count()
        };
        assert_eq!(count("manufacturer"), 2);
        assert_eq!(count("distributor"), 1);
        assert_eq!(count("logistics"), 1);
        assert_eq!(count("pharmacy"), 3);
        assert_eq!(count("regulator"), 1);
        assert_eq!(count("certifier"), 1);
        let evils: Vec<&Company> = companies.iter().filter(|company| company.evil).collect();
        assert_eq!(evils.len(), 3);
        // Odd evil index → verifier-less certifier; even → verified manufacturer.
        assert_eq!(evils[0].role, "manufacturer");
        assert!(evils[0].has_verifier);
        assert_eq!(evils[1].role, "certifier");
        assert!(!evils[1].has_verifier);
        assert_eq!(evils[2].role, "manufacturer");
        assert!(evils[2].has_verifier);
    }

    #[test]
    fn run_state_caps_feed_security_and_offers() {
        let mut state = RunState::default();
        for index in 0..FEED_CAP + 5 {
            state.push_feed(format!("f{index}"));
        }
        assert_eq!(state.feed.len(), FEED_CAP);
        assert_eq!(state.feed[0].label, "f5");

        for index in 0..SECURITY_CAP + 3 {
            state.push_security(security_event(&format!("a{index}")));
        }
        assert_eq!(state.security.len(), SECURITY_CAP);
        assert_eq!(state.security[0].actor, "a3");

        for index in 0..OFFER_CAP + 4 {
            state.push_offer(offer_event(&format!("o{index}")));
        }
        assert_eq!(state.offers.len(), OFFER_CAP);
        assert_eq!(state.offers[0].tx_id, "o4");
    }

    #[test]
    fn record_tx_keeps_the_last_forty_and_counts_edges() {
        let mut state = RunState::default();
        state.record_tx("t1".into(), "kind", "a", "b", "l1".into());
        state.record_tx("t2".into(), "kind", "a", "c", "l2".into());
        // A shared `from` must not fold two distinct edges together.
        assert_eq!(state.edges.len(), 2);
        assert_eq!(state.edges[0].count, 1);
        assert_eq!(state.edges[1].to, "c");

        for index in 0..40 {
            state.record_tx(format!("t{index}"), "kind", "x", "y", "l".into());
        }
        assert_eq!(state.transactions.len(), 40);
        assert_eq!(state.transactions[0].id, "t0");
        assert_eq!(state.transactions[39].id, "t39");
    }

    #[test]
    fn record_commit_caps_the_percentile_window() {
        let mut state = RunState::default();
        for index in 0..COMMIT_WINDOW + 4 {
            state.record_commit(u64::try_from(index).expect("fits"), 0, 0);
        }
        assert_eq!(state.commit_times.len(), COMMIT_WINDOW);
        assert_eq!(state.commit_times[0], 4, "the newest window is kept");
        assert_eq!(state.metrics.blocks, (COMMIT_WINDOW + 4) as u64);
    }

    #[test]
    fn refresh_wms_recomputes_stock_and_low_stock() {
        let mut state = RunState::default();
        let row = |company: &str| WmsRow {
            company: company.into(),
            role: "pharmacy".into(),
            product: "SKU-DEMO".into(),
            ..WmsRow::default()
        };
        state.wms.push(row("pharmacy-1"));
        state.wms.push(row("pharmacy-2"));
        state.wms.push(row("pharmacy-3"));
        state.wms.push(row("pharmacy-4"));
        state.inventory.insert("pharmacy-1".into(), 199);
        state.inventory.insert("pharmacy-2".into(), 199);
        state.inventory.insert("pharmacy-3".into(), 200);
        state.inventory.insert("pharmacy-4".into(), 10_000);
        // system_stock 10_598 → pressure 2 → low-stock threshold 200; the two
        // 199 rows are low, the 200 and 10 000 rows are not.
        assert_eq!(buy_pressure(&state), 2);
        state.refresh_wms();
        assert_eq!(state.wms[0].sellable_units, 199);
        assert_eq!(state.wms[2].sellable_units, 200);
        assert_eq!(
            state.wms_summary.low_stock, 2,
            "strictly below 200, not equal to or above it"
        );
        assert_eq!(state.wms_summary.members, 4);
    }

    #[test]
    fn wms_move_creates_rows_and_only_credits_positive_receipts() {
        let mut state = RunState::default();
        state.wms_move("pharmacy-1", "pharmacy", 5, 2);
        assert_eq!(state.wms.len(), 1);
        assert_eq!(state.wms[0].received_units, 5);
        assert_eq!(state.wms[0].dispatched_units, 2);
        assert_eq!(state.sellable("pharmacy-1"), 5);

        // A zero-receipt move must not create an inventory entry.
        state.wms_move("pharmacy-2", "pharmacy", 0, 3);
        assert_eq!(state.wms.len(), 2);
        assert!(!state.inventory.contains_key("pharmacy-2"));
    }

    #[test]
    fn sellable_reads_the_inventory_map() {
        let mut state = RunState::default();
        assert_eq!(state.sellable("nobody"), 0);
        state.inventory.insert("pharmacy-1".into(), 7);
        assert_eq!(state.sellable("pharmacy-1"), 7);
    }

    #[test]
    fn refresh_throughput_uses_whole_elapsed_seconds() {
        let mut state = RunState {
            started_ms: Some(now_millis().saturating_sub(2_000)),
            metrics: Metrics {
                submitted: 10,
                ..Metrics::default()
            },
            ..RunState::default()
        };
        state.refresh_throughput();
        assert_eq!(state.metrics.elapsed_s, 2);
        assert_eq!(state.metrics.tx_per_sec, 5);

        // Without a start stamp the window is zero and the 1-second floor
        // keeps throughput finite.
        let mut fresh = RunState::default();
        fresh.metrics.submitted = 3;
        fresh.refresh_throughput();
        assert_eq!(fresh.metrics.elapsed_s, 0);
        assert_eq!(fresh.metrics.tx_per_sec, 3);
    }

    #[test]
    fn payload_summary_describes_every_kind() {
        assert!(payload_summary(&serde_json::json!({
            "kind": "storage", "warehouse": "DC-x", "temp_range": "2-8",
            "humidity": "60", "retention_days": 90
        }))
        .contains("DC-x"));
        assert!(payload_summary(&serde_json::json!({
            "kind": "transit", "route": "mfg → hub", "transit_hours": 36,
            "cold_chain": "continuous", "delivery_window": "next day"
        }))
        .contains("mfg → hub"));
        assert!(payload_summary(&serde_json::json!({
            "kind": "process", "steps": ["mixing", "filling"],
            "batch_record": "BR-1", "gmp_line": "line-1", "operator_shift": "B"
        }))
        .contains("mixing → filling"));
        assert!(payload_summary(&serde_json::json!({
            "kind": "intake", "checks": ["seal integrity"], "quarantine_hours": 24
        }))
        .contains("seal integrity"));
        assert!(payload_summary(&serde_json::json!({
            "kind": "temperature_log", "sensor": "TS-1", "min_c": 2.0,
            "max_c": 7.5, "samples_per_hour": 4, "excursions": 0
        }))
        .contains("TS-1"));
        assert!(payload_summary(&serde_json::json!({
            "kind": "certification_evidence", "findings": "conforms",
            "samples_tested": 12, "lab_reference": "LAB-1", "next_audit": "2027"
        }))
        .contains("LAB-1"));
        assert!(payload_summary(&serde_json::json!({
            "kind": "regulator_notes", "inspection": "routine",
            "finding": "none", "inspector": "officer"
        }))
        .contains("officer"));
        // The fallback (pricing/terms) keeps the negotiated terms readable.
        assert!(payload_summary(&serde_json::json!({
            "kind": "pricing", "member_price_per_unit": 875,
            "list_price_per_unit": 1_000, "quantity": 5,
            "currency": "USD", "payment_terms": "net 60"
        }))
        .contains("net 60"));
    }

    #[test]
    fn offer_price_ladder_is_exact() {
        assert_eq!(offer_price(0), 1_350);
        assert_eq!(offer_price(1), 1_100);
        assert_eq!(offer_price(2), 950);
        assert_eq!(offer_price(3), 1_500);
        assert_eq!(offer_price(5), 950);
        assert_eq!(offer_price(7), 1_050);
    }

    #[test]
    fn offer_quantity_tiers_and_pressure_are_exact() {
        assert_eq!(offer_quantity(0, 0), 250);
        assert_eq!(offer_quantity(3, 1), 350);
        assert_eq!(offer_quantity(1, 2), 500);
        // Seq 35 prices exactly at the auto cap: it stays on the auto tiers.
        assert_eq!(offer_price(35), CONTRACT_MAX_PRICE);
        assert_eq!(offer_quantity(35, 0), 150);
        assert_eq!(offer_quantity(7, 0), 100);
    }

    #[test]
    fn contract_bands_and_buyers_are_exact() {
        assert_eq!(manual_contract_for_price(2_500), PREMIUM_CONTRACT_ID);
        assert_eq!(manual_contract_for_price(2_000), MANUAL_CONTRACT_ID);
        assert_eq!(manual_contract_for_price(1_700), MANUAL_CONTRACT_ID);
        assert_eq!(manual_contract_for_price(1_600), VALUE_CONTRACT_ID);
        assert_eq!(manual_contract_for_price(1_000), VALUE_CONTRACT_ID);

        assert_eq!(offer_buyer(2_500), "pharmacy-3");
        assert_eq!(offer_buyer(1_700), "pharmacy-1");
        assert_eq!(offer_buyer(1_000), "pharmacy-2");

        assert_eq!(manual_contract_for_buyer("pharmacy-2"), VALUE_CONTRACT_ID);
        assert_eq!(manual_contract_for_buyer("pharmacy-3"), PREMIUM_CONTRACT_ID);
        assert_eq!(manual_contract_for_buyer("pharmacy-1"), MANUAL_CONTRACT_ID);
    }

    #[test]
    fn stock_pressure_tracks_the_fleet_stock() {
        let mut state = RunState::default();
        assert_eq!(system_stock(&state), 0);
        assert_eq!(buy_pressure(&state), 1);
        state.inventory.insert("a".into(), 100);
        assert_eq!(system_stock(&state), 100);
        assert_eq!(buy_pressure(&state), 1, "a small stock is still scarce");
        state.inventory.insert("b".into(), 4_900);
        assert_eq!(system_stock(&state), 5_000);
        assert_eq!(buy_pressure(&state), 2);
        state.inventory.insert("c".into(), 50_000);
        assert_eq!(
            buy_pressure(&state),
            PRESSURE_CAP,
            "capped at the flush end"
        );
    }

    #[test]
    fn supply_offer_derives_price_quantity_and_lead_time() {
        let offer = |seq: u64, pressure: u64| {
            let TransactionKind::SupplyOffer(offer) = supply_offer(seq, "seller-1", pressure).kind
            else {
                panic!("expected a SupplyOffer");
            };
            offer
        };
        let quote = offer(3, 1);
        assert_eq!(quote.price_per_unit, offer_price(3));
        assert_eq!(quote.quantity_available, offer_quantity(3, 1));
        assert_eq!(quote.lead_time_days, 6, "3 + 3 % 4");
        assert_eq!(quote.seller_id, "seller-1");
        assert_eq!(offer(7, 0).lead_time_days, 6, "3 + 7 % 4");
    }

    #[test]
    fn pricing_terms_apply_the_member_discount() {
        let value: serde_json::Value =
            serde_json::from_str(&pricing_terms(1, 2, "manufacturer-1")).expect("json");
        let list = offer_price(1);
        assert_eq!(value["list_price_per_unit"].as_u64(), Some(list));
        assert_eq!(
            value["member_price_per_unit"].as_u64(),
            Some(list - list / 8)
        );
        assert_eq!(value["quantity"].as_u64(), Some(offer_quantity(1, 2)));
        assert_eq!(value["author"].as_str(), Some("manufacturer-1"));
    }

    #[test]
    fn intake_and_temperature_terms_carry_their_kinds() {
        let intake: serde_json::Value =
            serde_json::from_str(&intake_checklist(1, "distributor-1")).expect("json");
        assert_eq!(intake["kind"].as_str(), Some("intake"));
        assert_eq!(intake["quarantine_hours"].as_u64(), Some(24));
        assert!(intake["checks"][0].as_str().is_some());

        let temperature: serde_json::Value =
            serde_json::from_str(&temperature_log_terms(1, "logistics-1")).expect("json");
        assert_eq!(temperature["kind"].as_str(), Some("temperature_log"));
        assert_eq!(temperature["sensor"].as_str(), Some("TS-0001"));
    }

    #[test]
    fn retail_sale_is_a_negative_inventory_delta() {
        let TransactionKind::InventoryUpdate(update) = retail_sale_tx("pharmacy-1", 40, 2).kind
        else {
            panic!("expected an InventoryUpdate");
        };
        assert_eq!(update.quantity_delta, -40);
        assert_eq!(update.owner_id, "pharmacy-1");
    }

    #[tokio::test]
    async fn apply_params_bumps_topology_only_when_a_count_changes() {
        let shared = SharedRun::default();
        let before = shared
            .topology_version
            .load(std::sync::atomic::Ordering::SeqCst);

        // Every topology field, changed alone, must bump the version: this
        // pins each `||` and each `!=` in the change test.
        let mut base = SimParams::defaults();
        for mutate in [
            |params: &mut SimParams| params.manufacturers += 1,
            |params: &mut SimParams| params.distributors += 1,
            |params: &mut SimParams| params.logistics += 1,
            |params: &mut SimParams| params.pharmacies += 1,
            |params: &mut SimParams| params.regulators += 1,
            |params: &mut SimParams| params.certifiers += 1,
            |params: &mut SimParams| params.evil_nodes += 1,
        ] {
            let mut requested = base.clone();
            mutate(&mut requested);
            base = apply_params(&shared, requested).await;
        }
        let after = shared
            .topology_version
            .load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(after, before + 7, "each topology change bumped the version");

        // A timing-only edit does not rebuild the federation.
        let mut timing = base.clone();
        timing.round_interval_ms = 123;
        let applied = apply_params(&shared, timing).await;
        assert_eq!(applied.round_interval_ms, 123);
        assert_eq!(
            shared
                .topology_version
                .load(std::sync::atomic::Ordering::SeqCst),
            after,
            "timing-only edits do not bump the topology"
        );
    }

    #[tokio::test]
    async fn params_view_echoes_the_stored_params() {
        let shared = SharedRun::default();
        let requested = SimParams {
            round_interval_ms: 777,
            lots_per_round: 9,
            ..SimParams::defaults()
        };
        let applied = apply_params(&shared, requested).await;
        assert_eq!(applied.round_interval_ms, 777);
        assert_eq!(params_for_view(&shared).await.round_interval_ms, 777);
        assert_eq!(params_for_view(&shared).await.lots_per_round, 9);
    }

    #[test]
    fn ms_since_measures_elapsed_millis() {
        let start = Instant::now();
        std::thread::sleep(Duration::from_millis(20));
        let elapsed = ms_since(start);
        assert!((15..5_000).contains(&elapsed), "{elapsed}");
    }

    #[test]
    fn now_millis_tracks_the_wall_clock() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64;
        let value = now_millis();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64;
        assert!(
            value >= before && value <= after,
            "{value} outside [{before}, {after}]"
        );
    }

    #[tokio::test]
    async fn complete_purchase_rejects_an_unknown_offer() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let error = complete_purchase(&shared, "no-such-offer", "pharmacy-1", 1)
            .await
            .expect_err("an unknown offer must be rejected");
        assert!(error.contains("offer not found"), "{error}");
    }

    #[tokio::test]
    async fn evil_attacks_count_every_refusal_and_admit_the_under_metadata() {
        let shared = Arc::new(SharedRun::default());
        let _leader = setup_run(&shared).await;
        // `setup_run` seeds the schema identity before any projection runs.
        {
            let state = shared.state.lock().await;
            assert_eq!(
                state.compliance.schema_version,
                format!("v{SCHEMA_VERSION_V1}")
            );
            assert_eq!(state.compliance.fields_total, 6);
        }
        let companies = build_companies(&shared.params.lock().await.clone());
        let evil_count = companies.iter().filter(|company| company.evil).count() as u64;
        assert!(evil_count > 0);

        let before = shared.state.lock().await.metrics.rejected;
        let admitted = evil_attacks(&shared, &companies, 1).await;
        let after = shared.state.lock().await.metrics.rejected;
        // Four of the five attacks per evil node are refused and counted; the
        // fifth (under-metadata registration) is admitted by design.
        assert_eq!(after - before, 4 * evil_count);
        assert_eq!(admitted.len() as u64, evil_count);
    }
}
