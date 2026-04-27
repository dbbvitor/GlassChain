//! High-level Rust-native SDK for `GlassChain`.
//!
//! # Quick Start
//!
//! Register an asset in under 10 lines:
//!
//! ```rust,no_run
//! use glasschain_sdk::GlasschainClient;
//! use glasschain_core::TraceableAsset;
//!
//! # fn main() -> Result<(), glasschain_sdk::SdkError> {
//! let asset = TraceableAsset {
//!     gtin: Some("07891234567890".into()),
//!     batch_number: Some("LOTE-001".into()),
//!     expiry_date: Some("2027-12-31".into()),
//!     serial_number: Some("SN-00001".into()),
//!     anvisa_registration: Some("MS 1.0000.0001.001-1".into()),
//!     manufacturer_id: Some("12.345.678/0001-99".into()),
//!     product_name: "Dipirona 500mg".into(),
//!     custodian_id: "my-node".into(),
//!     country_of_origin: Some("BR".into()),
//!     storage_temp_celsius: Some("15-30".into()),
//!     quantity: 1000,
//! };
//! let tx_json = GlasschainClient::build_asset_registration_tx(
//!     "my-node",
//!     asset,
//!     "MANUFACTURE",
//! )?;
//! println!("Submit this JSON to gRPC SubmitTransaction: {tx_json}");
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod error;

pub use client::{ChainStatus, GlasschainClient, GlasschainClientConfig};
pub use error::SdkError;
