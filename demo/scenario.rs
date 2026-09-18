// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! The synthetic demo run: a real in-process GlassChain federation whose
//! topology comes from operator-editable parameters — multiple companies per
//! role, evil nodes attacking the real fail-closed paths, contract-driven
//! offer matching, and per-member visibility through their own nodes.
//! Presentation only — never production evidence.

use glasschain_core::{
    capability_hash, CanonicalRecord, CapabilityActivation, InventoryUpdate, MetadataTrustScore,
    PurchaseConditions, PurchaseOrder, RecordSignature, SmartContractDef, SupplyOffer,
    TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
};
use glasschain_identity::{CertChainVerifier, Channel, ChannelConfig, Organization};
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
/// Conditions-only contract (auto_execute=false): offers priced above the
/// auto cap match only this one and wait for a human buyer decision.
const MANUAL_CONTRACT_ID: &str = "manual-review";
/// Demo starting cash per company (minor units: $100 000). Presentation
/// bookkeeping — the chain itself carries no currency balance.
const CASH_START: u64 = 10_000_000;
const MANUAL_MAX_PRICE: u64 = 2_000;
const CONTRACT_MAX_PRICE: u64 = 1_200;
const CONTRACT_MAX_QTY: u64 = 1_000_000;
/// The lots table never sheds rows during a demo session — the user asked
/// for a stable, append-only history. The cap only guards extreme runs.
const LOTS_CAP: usize = 500;
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
        self.manufacturers = self.manufacturers.clamp(1, 4);
        self.distributors = self.distributors.clamp(1, 3);
        self.logistics = self.logistics.clamp(1, 3);
        self.pharmacies = self.pharmacies.clamp(1, 4);
        self.regulators = self.regulators.clamp(0, 2);
        self.certifiers = self.certifiers.clamp(1, 3);
        self.evil_nodes = self.evil_nodes.clamp(0, 3);
        self.lots_per_round = self.lots_per_round.clamp(1, 10);
        self.round_interval_ms = self.round_interval_ms.clamp(100, 5_000);
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
    let mut counter = 0u8;
    let mut push_role = |count: u8, role: &str, companies: &mut Vec<Company>| {
        for index in 1..=count {
            companies.push(Company {
                id: format!("{role}-{index}"),
                role: role.to_owned(),
                evil: false,
                has_verifier: true,
            });
            counter += 1;
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
        counter += 1;
    }
    let _ = counter;
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

#[derive(Serialize, Clone)]
pub struct OrgView {
    pub id: String,
    pub role: String,
    pub evil: bool,
    /// Collections this org is a member of; empty for certifiers and evils.
    pub member_of: Vec<String>,
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
    pub pool_count: u64,
    pub pool_bytes: u64,
}

#[derive(Serialize, Clone)]
pub struct FeedItem {
    pub label: String,
    pub height: u64,
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
    /// Run start (ms since epoch) for throughput math. Not shown raw.
    #[serde(skip)]
    started_ms: Option<u64>,
    /// Rolling commit latencies (percentile source, not shown raw).
    #[serde(skip)]
    commit_times: Vec<u64>,
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
}

/// Payloads disseminated by the runner: `(collection, commitment, cleartext)`.
pub type PayloadLedger = Vec<(String, String, String)>;

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
/// company) is the block producer; everyone dials it.
async fn build_federation(companies: &[Company], leader: &str) -> Nodes {
    let mut org = Organization::new("GlassChain Demo").expect("demo root org builds");
    let verifier = verifier_for(&org).await;
    let mut nodes: Nodes = BTreeMap::new();

    // Concrete loopback ports: `listen_addr()` echoes the *configured*
    // string, so a wildcard `:0` would make peers dial port 0. Reserve each
    // socket first (the repo's prebound-listener pattern).
    let leader_addr = reserve_addr();
    for company in companies {
        let identity = org
            .issue_identity(company.id.clone())
            .expect("demo identity mints")
            .clone();
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
    // Allow the handshake wave + first sync; scale the settle with the fleet.
    tokio::time::sleep(Duration::from_millis(
        500 + 80 * u64::from(companies.len() as u32),
    ))
    .await;
    nodes
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

fn replenish_contract(buyer: &str) -> Transaction {
    Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
        contract_id: CONTRACT_ID.into(),
        buyer_id: buyer.into(),
        product_id: "SKU-DEMO".into(),
        conditions: PurchaseConditions {
            max_price_per_unit: CONTRACT_MAX_PRICE,
            min_quantity: 1,
            max_quantity: CONTRACT_MAX_QTY,
            max_lead_time_days: 30,
            preferred_seller_id: None,
            currency: "USD".into(),
            auto_execute: true,
        },
        wasm_code_b64: None,
    }))
}

/// The offer price ladder: two ordinary rungs under the `auto-replenish`
/// cap, one premium rung above it (manual buyers only).
fn offer_price(seq: u64) -> u64 {
    if seq.is_multiple_of(3) {
        1_400
    } else {
        900 + (seq % 3) * 100
    }
}

/// Each lot is 500 units; the ladder (300 / 400 / 500) makes auto
/// purchases partial lot buys with the remainder in the warehouse. The
/// multiplier is the stock-driven buy pressure: scarce stock keeps the
/// ladder partial, piled-up stock buys full lots so goods reach the
/// retail drain faster.
fn offer_quantity(seq: u64, pressure: u64) -> u64 {
    let base = 300 + (seq % 3) * 100;
    (base * pressure).min(500)
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
            originator_id: "manufacturer-1".into(),
            purchase_order_ref: None,
        },
    ))
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
        contract_id: Some(MANUAL_CONTRACT_ID.into()),
    }));
    let order_id = order.id.clone();
    let node = node_for(shared, buyer).await;
    node.submit_transaction(order)
        .await
        .map_err(|error| format!("admission rejected: {error}"))?;
    {
        eprintln!("debug: purchase state lock");
        let mut state = shared.state.lock().await;
        eprintln!("debug: purchase state locked");
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
        state.push_offer(OfferEvent {
            kind: "purchase".into(),
            tx_id: order_id.clone(),
            seller,
            buyer: buyer.to_owned(),
            product: offer.product.clone(),
            quantity,
            sold: 0,
            price_per_unit: offer.price_per_unit,
            note: format!(
                "manually completed by {buyer} against {MANUAL_CONTRACT_ID} (committing next                  block)"
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
    let pending: std::collections::HashSet<String> = {
        let ledger = leader.shared_ledger();
        let ledger = ledger.lock().await;
        ledger
            .pending_transactions
            .iter()
            .map(|tx| tx.id.clone())
            .collect()
    };
    let missing: Vec<&String> = ids.iter().filter(|id| !pending.contains(*id)).collect();
    if missing.is_empty() {
        return;
    }
    let deadline = Instant::now() + Duration::from_millis(2_500);
    loop {
        let ledger = leader.shared_ledger();
        let ledger = ledger.lock().await;
        let present: std::collections::HashSet<String> = ledger
            .pending_transactions
            .iter()
            .map(|tx| tx.id.clone())
            .collect();
        drop(ledger);
        if ids.iter().all(|id| present.contains(id)) || Instant::now() > deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn submit(node: &Arc<Node>, tx: Transaction, state: &mut RunState) {
    match node.submit_transaction(tx).await {
        Ok(()) => state.metrics.submitted += 1,
        Err(error) => {
            state.metrics.rejected += 1;
            state.push_feed(format!("admission rejected: {error}"));
        }
    }
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
    let nodes = build_federation(&companies, &leader_id).await;
    for node in nodes.values() {
        node.set_collections(vec![pricing_collection(&companies)])
            .await;
    }
    {
        let mut state = shared.state.lock().await;
        state.status = "running".into();
        state.started_ms = Some(now_millis());
        state.params = params.clone();
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
            })
            .collect();
    }
    *shared.nodes.lock().await = nodes;
    let leader = node_for(shared, &leader_id).await;
    let activation = activation_tx(2);
    let _ = leader.submit_transaction(activation).await;
    // The buyer side of the contracts: the first pharmacy. Two contracts on
    // the same product — `auto-replenish` (auto-execute up to the price
    // cap) and `manual-review` (conditions-only, auto_execute=false): offers
    // above the auto cap match only the manual contract and wait for a
    // human buyer.
    let buyer = "pharmacy-1".to_owned();
    let _ = leader.submit_transaction(replenish_contract(&buyer)).await;
    let mut manual = replenish_contract(&buyer);
    if let TransactionKind::ContractCreation(ref mut def) = manual.kind {
        def.contract_id = MANUAL_CONTRACT_ID.into();
        def.conditions.max_price_per_unit = MANUAL_MAX_PRICE;
        def.conditions.auto_execute = false;
    }
    let _ = leader.submit_transaction(manual).await;
    // Both must be committed before private payloads are legal, so settle
    // them in their own block here.
    let _ = leader.mine().await;
    leader
}

// ── The round loop ──────────────────────────────────────────────────────────

/// One synchronized round: `lots_per_round` lots across rotating real
/// companies, supply offers matched by the real contract engine, both evil
/// companies attacking, one block per round.async fn drive_round(shared: &SharedRun, leader: &Arc<Node>, round: u64) {
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
    let buyer = "pharmacy-1".to_owned();
    let round_start = Instant::now();
    let mut round_txs: Vec<(Arc<Node>, Transaction)> = Vec::new();

    // ── 1. Advance the pipeline: every in-flight lot moves ONE custody hop
    // per round, so goods genuinely rest at each member between rounds —
    // every member holds stock, not just the end of the chain.
    let mut cert_txs: Vec<(Arc<Node>, Transaction)> = Vec::new();
    {
        let mut pipeline = shared.pipeline.lock().await;
        for entry in pipeline.iter_mut() {
            if entry.stage >= 3 && !entry.certified {
                // The lot just reached the pharmacy: certification + audit
                // from its certifier, riding the same block as everything
                // else.
                let record_id = format!("cert-{}", entry.seq);
                let record =
                    cert_record(entry.seq, "quality_certification", record_id, &entry.certifier);
                round_txs.push((
                    node_for(shared, &entry.certifier).await,
                    signed(record, &entry.certifier),
                ));
                let audit_id = format!("audit-{}", entry.seq);
                let audit = cert_record(
                    entry.seq,
                    "audit_attestation",
                    audit_id,
                    &entry.certifier,
                );
                round_txs.push((node_for(shared, &entry.certifier).await, signed(audit, &entry.certifier)));
                let mut state = shared.state.lock().await;
                state.certs.push(CertView {
                    record_id: format!("cert-{}", entry.seq),
                    schema: "quality_certification".into(),
                    lot_ref: format!("lot-{}", entry.seq),
                    issuer: entry.certifier.clone(),
                    status: "valid".into(),
                });
                state.certs.push(CertView {
                    record_id: format!("audit-{}", entry.seq),
                    schema: "audit_attestation".into(),
                    lot_ref: format!("lot-{}", entry.seq),
                    issuer: entry.certifier.clone(),
                    status: "valid".into(),
                });
                if state.certs.len() > 30 {
                    state.certs.remove(0);
                }
                entry.certified = true;
            }
            if entry.stage < 3 {
                // One hop this round: dispatch (mfg→dist, dist→logx) or
                // receive (logx→pharmacy), submitted by the handover member.
                let hops: [(&str, &str, &str); 3] = [
                    ("dispatch", entry.manufacturer.as_str(), entry.distributor.as_str()),
                    ("dispatch", entry.distributor.as_str(), entry.logistics.as_str()),
                    ("receive", entry.logistics.as_str(), entry.pharmacy.as_str()),
                ];
                let (stage, _from, to) = hops[entry.stage as usize];
                let tx = custody_tx(entry.seq, stage, to, false);
                let tx_id = tx.id.clone();
                round_txs.push((node_for(shared, to).await, tx));
                let mut state = shared.state.lock().await;
                state.record_tx(
                    tx_id.clone(),
                    "custody",
                    HOP_FROM[entry.stage as usize],
                    to,
                    format!("AssetRegistration `{stage}` → {to} (lot {})", entry.seq),
                );
                // WMS movement ledger: goods-in at the receiver, dispatch
                // out at the handover, sellable inventory moves too.
                state.wms_move(HOP_FROM[entry.stage as usize], "holder", 0, 500);
                state.wms_move(to, "holder", 500, 0);
                drop(state);
                entry.stage += 1;
            }
        }
        // Retire fully-certified entries — their lot rows persist.
        pipeline.retain(|entry| !(entry.stage >= 3 && entry.certified));
    }

    // ── 2. New lots enter the pipeline: anchor + manufacture this round,
    // downstream hops on later rounds.
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
                "lot {seq}: {manufacturer} manufactures; {distributor} → {logistics} →                  {pharmacy} queued (cert {certifier})"
            ));
            state.count_edge(manufacturer, distributor);
            state.count_edge(certifier, pharmacy);
        }

        // Lot anchor + manufacture, submitted this round.
        round_txs.push((
            node_for(shared, manufacturer).await,
            signed(lot_record(seq, manufacturer), manufacturer),
        ));
        {
            let mut state = shared.state.lock().await;
            state.record_tx(
                format!("lot-{seq}"),
                "lot",
                manufacturer,
                distributor,
                format!("Canonical `lot` anchor {seq} from {manufacturer}"),
            );
        }
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
            state.wms_move(manufacturer, "manufacturer", 500, 0);
        }
        round_txs.push((node_for(shared, manufacturer).await, manufacture));

        // The sell side: a SupplyOffer submitted by the manufacturer; the
        // buyer-side contract engine on that node matches it. Offers under
        // the `auto-replenish` cap auto-execute; premium offers are
        // advertised and wait for a human buyer.
        let offer_tx = supply_offer(seq, manufacturer, pressure);
        let offer_tx_id = offer_tx.id.clone();
        round_txs.push((node_for(shared, manufacturer).await, offer_tx));
        {
            let mut state = shared.state.lock().await;
            state.push_offer(OfferEvent {
                kind: "offer".into(),
                tx_id: offer_tx_id.clone(),
                seller: manufacturer.to_owned(),
                buyer: buyer.clone(),
                product: "SKU-DEMO".into(),
                quantity: offer_quantity(seq, pressure),
                sold: 0,
                price_per_unit: offer_price(seq),
                note: if offer_price(seq) > CONTRACT_MAX_PRICE {
                    format!(
                        "matched manual-review — {} of the lot's 500 units on offer, \
                         awaiting a buyer decision",
                        offer_quantity(seq, pressure)
                    )
                } else {
                    format!(
                        "partial lot offer: {} of the lot's 500 units (auto contract)",
                        offer_quantity(seq, pressure)
                    )
                },
            });
            state.record_tx(
                offer_tx_id,
                "offer",
                manufacturer,
                &buyer,
                format!("SupplyOffer {seq} from {manufacturer}"),
            );
            state.count_edge(manufacturer, &buyer);
        }

        // Member-only pricing payload from the manufacturer.
        let manufacturer_node = node_for(shared, manufacturer).await;
        let payload = format!("pricing-terms-{seq}");
        if manufacturer_node
            .submit_private_payload(COLLECTION, payload.as_bytes().to_vec())
            .await
            .is_ok()
        {
            let commitment = glasschain_core::crypto::sha256(payload.as_bytes());
            shared
                .payloads
                .lock()
                .await
                .push((COLLECTION.into(), commitment.clone(), payload));
            let mut state = shared.state.lock().await;
            if state.pdc.iter().all(|entry| entry.collection != COLLECTION) {
                state.pdc.push(PdcEntry {
                    collection: COLLECTION.into(),
                    commitments: Vec::new(),
                });
            }
            let entry = state
                .pdc
                .iter_mut()
                .find(|entry| entry.collection == COLLECTION)
                .expect("entry just ensured");
            entry.commitments.push(commitment);
            for member in pricing_member_ids(&companies) {
                if member != manufacturer {
                    state.count_edge(manufacturer, &member);
                }
            }
        }

        {
            let mut state = shared.state.lock().await;
            let trust = MetadataTrustScore::compute(&asset(manufacturer, seq, false));
            state.lots.push(LotView {
                lot_ref: format!("LOT-{seq}"),
                status: "manufactured".into(),
                manufacturer: manufacturer.to_owned(),
                trust_score: trust.score,
                chain: Vec::new(),
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

    // Evil attempts: recorded outcomes, whatever the node decides.
    let evil_ids = evil_attacks(shared, &companies, round).await;
    for (node, tx) in round_txs.drain(..) {
        let mut state = shared.state.lock().await;
        submit(&node, tx, &mut state).await;
    }

    // Relay + mine: wait until every submission of this round reached the
    // star center's pool (contract-generated PurchaseOrders may trail and
    // land in the next block — admission vs commit stays honest).
    let mut expected_ids: Vec<String> = {
        let pipeline = shared.pipeline.lock().await;
        pipeline.iter().map(|entry| format!("lot-{}", entry.seq)).collect()
    };
    for (node, tx) in round_txs.drain(..) {
        let mut state = shared.state.lock().await;
        submit(&node, tx, &mut state).await;
    }
    expected_ids.extend(evil_ids);
    wait_for_ids(leader, &expected_ids).await;
    let mine_start = Instant::now();
    let _ = leader.mine().await;
    let commit_ms = u64::try_from(mine_start.elapsed().as_millis()).unwrap_or(u64::MAX);

    // Honest receipt: custody chains from the provenance index, plus the
    // engine-generated PurchaseOrders (the matches) that landed in the
    // committed block.
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
                    let manual = order.contract_id.as_deref() == Some(MANUAL_CONTRACT_ID);
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
                                    "auto-executed: {0} of the lot's 500 units bought, \
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
                        note: if manual {
                            format!(
                                "manually completed by {} against {MANUAL_CONTRACT_ID}, tx {}",
                                order.buyer_id, tx.id
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

    // Custody chain receipt: the provenance index tells us where every
    // in-flight lot physically sits.
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
                    index.get_custody_chain(&asset_id(entry.seq, false)).to_vec(),
                )
            })
            .collect();
        (stage_by_seq, chains)
    };
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
    for (seq, events) in chains {
        let Some(view) = state
            .lots
            .iter_mut()
            .find(|lot| lot.lot_ref == format!("LOT-{seq}"))
        else {
            continue;
        };
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
    if state.lots.len() > LOTS_CAP {
        let over = state.lots.len() - LOTS_CAP;
        state.lots.drain(0..over);
    }
    state.refresh_wms();
    state.refresh_throughput();
    drop(state);

    // Retail phase: pharmacies sell from their own inventories to end
    // customers, a little every round — stock accumulates first, and only
    // sustained rounds drain a warehouse completely. Each sale is a real
    // committed InventoryUpdate (quietly, off the live graph).
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
    log::info!(
        "round {round}: commit {commit_ms} ms, whole round {} ms",
        round_start.elapsed().as_millis()
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

        // 3. under-metadata registration: admitted (zero trust is not
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
pub async fn org_snapshot(shared: &SharedRun, org: &str) -> Value {
    let node = shared.nodes.lock().await.get(org).cloned();
    let Some(node) = node else {
        return serde_json::json!({ "error": "unknown viewing org" });
    };
    let chain_height = node.shared_ledger().lock().await.chain.len() as u64;

    // Payload custody from the member's own transient store.
    let payloads = shared.payloads.lock().await.clone();
    let mut pdc_values = Vec::new();
    for (collection, commitment, cleartext) in &payloads {
        let held = node
            .transient_payload(collection, commitment)
            .await
            .is_some();
        if held {
            pdc_values.push(OrgPayload {
                collection: collection.clone(),
                commitment: commitment.clone(),
                payload: Some(cleartext.clone()),
            });
        }
    }
    // What this member holds in stock (committed custody), its demo cash
    // balance, and the offers where it is a counterparty.
    let state = shared.state.lock().await;
    let stock = state.wms.iter().find(|row| row.company == org);
    let cash = state.cash.get(org).copied().unwrap_or(CASH_START);
    let inventory = state.sellable(org);
    let sell_offers: Vec<&OfferEvent> = state
        .offers
        .iter()
        .filter(|event| event.kind == "offer" && event.seller == org)
        .collect();
    let purchases_as_buyer: Vec<&OfferEvent> = state
        .offers
        .iter()
        .filter(|event| event.kind == "purchase" && event.buyer == org)
        .collect();
    let sells: Vec<serde_json::Value> = sell_offers
        .iter()
        .map(|offer| {
            serde_json::json!({
                "product": offer.product,
                "quantity": offer.quantity,
                "sold": offer.sold,
                "price_per_unit": offer.price_per_unit,
                "note": offer.note,
            })
        })
        .collect();
    let buys: Vec<serde_json::Value> = purchases_as_buyer
        .iter()
        .map(|order| {
            serde_json::json!({
                "product": order.product,
                "quantity": order.quantity,
                "price_per_unit": order.price_per_unit,
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
        "cash": cash,
        "sell_offers": sells,
        "purchases": buys,
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
                        watcher_shared.state.lock().await.push_offer(OfferEvent {
                            kind: "execution".into(),
                            tx_id: String::new(),
                            seller: "—".into(),
                            buyer: "—".into(),
                            product: "—".into(),
                            quantity,
                            sold: 0,
                            price_per_unit: 0,
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
    let leader = setup_run(shared).await;
    drive_round(&shared, &leader, 1).await;
    drop(leader);
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_pressure_scales_with_absolute_stock() {
        // Scarce stock: the partial-lot ladder (300/400/500).
        let scarce = RunState {
            inventory: [
                ("manufacturer-1".to_owned(), 1_000u64),
                ("pharmacy-1".to_owned(), 1_000),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert_eq!(buy_pressure(&scarce), 1);
        assert_eq!(offer_quantity(0, 1), 300);
        assert_eq!(offer_quantity(1, 1), 400);
        assert_eq!(offer_quantity(2, 1), 500);
        // 10 000+ system units: full-lot buys (pressure capped at 2).
        let flush = RunState {
            inventory: [("manufacturer-1".to_owned(), 12_000u64)]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert_eq!(buy_pressure(&flush), PRESSURE_CAP);
        assert_eq!(offer_quantity(0, PRESSURE_CAP), 500);
        assert_eq!(offer_quantity(1, PRESSURE_CAP), 500);
        assert_eq!(offer_quantity(2, PRESSURE_CAP), 500);
    }

    #[test]
    fn build_companies_honors_the_parameters() {
        let params = SimParams {
            manufacturers: 2,
            distributors: 1,
            logistics: 1,
            pharmacies: 2,
            regulators: 0,
            certifiers: 1,
            evil_nodes: 3,
            lots_per_round: 2,
            round_interval_ms: 300,
        };
        let companies = build_companies(&params);
        assert_eq!(companies.len(), 10);
        assert_eq!(companies[0].id, "manufacturer-1");
        assert_eq!(companies[4].id, "pharmacy-1");
        // Honest roles in order: 2 mfg, 1 dist, 1 logx, 2 pharm, 0 reg, 1 cert.
        assert_eq!(companies[6].id, "certifier-1");
        // Evils alternate: evil-1 manufacturer (verified), evil-2 certifier
        // (no verifier), evil-3 manufacturer.
        assert!(companies[7].evil);
        assert_eq!(companies[7].id, "evil-1");
        assert_eq!(companies[7].role, "manufacturer");
        assert!(companies[7].has_verifier);
        assert!(!companies[8].has_verifier);
        assert_eq!(companies[8].role, "certifier");
        assert_eq!(companies[9].role, "manufacturer");
    }

    #[test]
    fn pricing_membership_excludes_evils_and_certifiers() {
        let params = SimParams::defaults();
        let companies = build_companies(&params);
        let members = pricing_member_ids(&companies);
        assert_eq!(members.len(), 11); // 3 mfg + 2 dist + 2 logx + 3 pharm + 1 regulator
        assert!(!members.iter().any(|m| m.starts_with("evil-")));
        assert!(!members.iter().any(|m| m.starts_with("certifier-")));
    }

    #[test]
    fn params_sanitize_and_bump_topology_only_on_topology_change() {
        let shared = SharedRun::default();
        // Same topology, different round shape: no rebuild.
        let applied = futures_block_on(apply_params(
            &shared,
            SimParams {
                lots_per_round: 7,
                ..SimParams::defaults()
            },
        ));
        assert_eq!(applied.lots_per_round, 7);
        assert_eq!(
            shared
                .topology_version
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        // New topology: rebuild signalled.
        let applied = futures_block_on(apply_params(
            &shared,
            SimParams {
                pharmacies: 9,
                ..SimParams::defaults()
            },
        ));
        assert_eq!(applied.pharmacies, 4); // clamped
        assert_eq!(
            shared
                .topology_version
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    fn futures_block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime builds")
            .block_on(future)
    }

    #[tokio::test]
    async fn one_round_drives_the_whole_scenario() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let state = shared.state.lock().await;
        assert_eq!(state.status, "running");
        assert_eq!(state.orgs.len(), build_companies(&state.params).len());
        assert!(state.metrics.blocks >= 1, "the round mined a block");
        let complete = state.lots.iter().any(|lot| lot.status == "complete");
        assert!(complete, "a lot reached the pharmacy through the chain");
        assert_eq!(state.pdc.len(), 1);
        let payloads = shared.payloads.lock().await;
        assert!(payloads.iter().any(
            |(collection, _, payload)| collection == COLLECTION && payload.contains("pricing-")
        ));
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
        run_one_round(&shared).await;
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
        assert!(
            sold <= RETAIL_UNITS_PER_ROUND * PRESSURE_CAP,
            "retail drains slowly and boundedly: {sold}"
        );
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
        run_one_round(&shared).await;
        let count = shared.state.lock().await.lots.len();
        // Three lots this round: all three must still be listed.
        assert_eq!(count, 3, "lots are append-only, never shed mid-session");
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
    async fn member_views_show_stock_cash_and_offers() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let view = org_snapshot(&shared, "manufacturer-1").await;
        assert!(
            view["stock"].is_object(),
            "the member's stock is in the view"
        );
        assert!(view["cash"].as_u64().expect("cash") > 0);
        assert!(
            !view["sell_offers"].as_array().expect("array").is_empty(),
            "the manufacturer's advertised sell offers are listed"
        );
        let pharmacy = org_snapshot(&shared, "pharmacy-1").await;
        assert!(pharmacy["stock"].is_object());
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
        // Nothing evil ever entered the pricing payloads.
        let payloads = shared.payloads.lock().await;
        assert!(payloads
            .iter()
            .all(|(_, _, payload)| payload.starts_with("pricing-terms-")));
    }

    #[tokio::test]
    async fn org_views_come_from_each_members_own_node() {
        let shared = Arc::new(SharedRun::default());
        run_one_round(&shared).await;
        let regulator = org_snapshot(&shared, "regulator-1").await;
        let certifier = org_snapshot(&shared, "certifier-1").await;
        let regulator_values = regulator["pdc_values"].as_array().expect("array");
        assert!(
            !regulator_values.is_empty(),
            "the regulator is a member and holds the payloads"
        );
        assert!(certifier["pdc_values"]
            .as_array()
            .expect("array")
            .is_empty());
        assert_eq!(regulator["chain_height"], certifier["chain_height"]);
    }
}
