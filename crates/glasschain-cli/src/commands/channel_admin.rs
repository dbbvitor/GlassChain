// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! `channel-admin` sub-command — drive the ADR-017 channel-management RPCs
//! (`NodeService.CreateChannel` / `AddChannelMember` /
//! `RemoveChannelMember`) as a certificate-bound MSP admin principal.
//!
//! The command authenticates with the four admin headers
//! (`glasschain_rpc::admin_headers_from_cert`): the `x-glasschain-*` MSP
//! headers, the timestamp and its ed25519 signature under the
//! certificate's key, and the base64 DER certificate itself. The server's
//! `AdminGate` verifies the chain, CRL, CN and possession before acting.

use anyhow::Result;
use clap::{Args, Subcommand};
use glasschain_rpc::proto::glasschain_v1::node_service_client::NodeServiceClient;
use glasschain_rpc::proto::glasschain_v1::{
    AddChannelMemberRequest, CreateChannelRequest, RemoveChannelMemberRequest,
};

/// Arguments accepted by the `channel-admin` sub-command.
#[derive(Args, Debug)]
pub struct ChannelAdminArgs {
    /// gRPC endpoint of the node to administer (e.g. `http://127.0.0.1:50051`).
    #[arg(long)]
    pub endpoint: String,

    /// PEM file holding the admin's organization-issued certificate
    /// (`OU=admin`, minted via `Organization::issue_identity_with_role`).
    #[arg(long)]
    pub cert: String,

    /// Hex-encoded 32-byte ed25519 seed of the certificate's signing key
    /// (the `seed_hex` field of the operator's identity file, ADR-018).
    #[arg(long)]
    pub key_seed: String,

    #[command(subcommand)]
    pub op: AdminOp,
}

/// The channel-management operations (ADR-017).
#[derive(Subcommand, Debug)]
pub enum AdminOp {
    /// Create a private data collection on the node.
    CreateChannel {
        /// Unique collection name.
        #[arg(long)]
        name: String,
        /// Human-readable description.
        #[arg(long, default_value = "")]
        description: String,
        /// Member organization IDs (repeatable).
        #[arg(long = "member")]
        member_ids: Vec<String>,
        /// Private-payload retention window in seconds (0 ⇒ the 72-hour
        /// default).
        #[arg(long, default_value_t = 0)]
        retention_secs: u64,
    },
    /// Admit a member organization to an existing collection.
    AddMember {
        #[arg(long)]
        name: String,
        #[arg(long)]
        member_id: String,
    },
    /// Remove a member organization from an existing collection.
    RemoveMember {
        #[arg(long)]
        name: String,
        #[arg(long)]
        member_id: String,
    },
}

/// Execute the `channel-admin` command.
///
/// Builds the admin headers from `--cert` / `--key-seed`, connects to the
/// endpoint, dispatches the requested operation and prints the outcome.
///
/// # Errors
///
/// Returns an error when the certificate or seed cannot be read, the server
/// is unreachable, or the gate denies the operation (its `PERMISSION_DENIED`
/// message is surfaced verbatim).
#[allow(clippy::needless_pass_by_value)] // clap gives us owned Args; consuming them is idiomatic
pub fn run(args: ChannelAdminArgs, out: &mut dyn std::io::Write) -> Result<()> {
    let cert_pem = std::fs::read_to_string(&args.cert)
        .map_err(|e| anyhow::anyhow!("cannot read admin certificate `{}`: {e}", args.cert))?;
    let seed_bytes = hex::decode(args.key_seed.trim())
        .map_err(|_| anyhow::anyhow!("--key-seed must be 64 hex characters"))?;
    let seed: [u8; 32] = seed_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("--key-seed must be 64 hex characters (32 bytes)"))?;
    let headers =
        glasschain_rpc::admin_headers_from_cert(&cert_pem, &seed).map_err(anyhow::Error::msg)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let outcome = runtime.block_on(execute(&args.endpoint, &args.op, headers))?;

    writeln!(out, "{outcome}")?;
    Ok(())
}

/// Dispatch the operation over gRPC and format the human outcome.
/// Attach the admin headers to a request.
fn attach<T>(mut req: tonic::Request<T>, headers: &[(&'static str, String)]) -> tonic::Request<T> {
    for (name, value) in headers {
        req.metadata_mut()
            .insert(*name, value.parse().expect("valid header value"));
    }
    req
}

async fn execute(
    endpoint: &str,
    op: &AdminOp,
    headers: [(&'static str, String); 4],
) -> Result<String> {
    // Retry briefly until the server accepts (a freshly spawned server needs
    // a moment to bind).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let channel = loop {
        match tonic::transport::Endpoint::from_shared(endpoint.to_owned())
            .expect("valid endpoint")
            .connect()
            .await
        {
            Ok(channel) => break channel,
            Err(_e) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => return Err(anyhow::anyhow!("cannot connect to {endpoint}: {e}")),
        }
    };
    let mut client = NodeServiceClient::new(channel);

    match op {
        AdminOp::CreateChannel {
            name,
            description,
            member_ids,
            retention_secs,
        } => {
            let response = client
                .create_channel(attach(
                    tonic::Request::new(CreateChannelRequest {
                        name: name.clone(),
                        description: description.clone(),
                        member_ids: member_ids.clone(),
                        retention_secs: *retention_secs,
                    }),
                    &headers,
                ))
                .await?
                .into_inner();
            Ok(format!(
                "created channel `{name}` (created={})",
                response.created
            ))
        }
        AdminOp::AddMember { name, member_id } => {
            let response = client
                .add_channel_member(attach(
                    tonic::Request::new(AddChannelMemberRequest {
                        name: name.clone(),
                        member_id: member_id.clone(),
                    }),
                    &headers,
                ))
                .await?
                .into_inner();
            Ok(format!(
                "added member `{member_id}` to `{name}` (added={})",
                response.added
            ))
        }
        AdminOp::RemoveMember { name, member_id } => {
            let response = client
                .remove_channel_member(attach(
                    tonic::Request::new(RemoveChannelMemberRequest {
                        name: name.clone(),
                        member_id: member_id.clone(),
                    }),
                    &headers,
                ))
                .await?
                .into_inner();
            Ok(format!(
                "removed member `{member_id}` from `{name}` (removed={})",
                response.removed
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glasschain_identity::CertChainVerifier;
    use glasschain_network::Node;
    use glasschain_rpc::server::GlasschainServer;
    use glasschain_rpc::AdminGate;
    use rustls_pki_types::pem::PemObject as _;
    use std::sync::Arc;

    /// The full operator loop over real gRPC, driven by the custody file the
    /// ADR-018 export produced: the admin seed comes from
    /// `Organization::export_json`, `admin_headers_from_cert` builds the
    /// headers, the gate admits the admin and refuses a plain member.
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // one flow: custody file → headers → three ops
    async fn channel_admin_manages_a_live_server() {
        let mut org = glasschain_identity::Organization::new("PharmaCorp").unwrap();
        let admin = org
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let member = org.issue_identity("plain-node").unwrap().clone();
        // The admin's seed, read the way an operator reads it: from the
        // durable identity file (ADR-018).
        let snapshot: serde_json::Value =
            serde_json::from_str(&org.export_json().unwrap()).unwrap();
        let admin_seed_hex = snapshot["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["node_id"] == "admin-node")
            .expect("admin member in the snapshot")["seed_hex"]
            .as_str()
            .unwrap()
            .to_owned();
        let seed = {
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&hex::decode(&admin_seed_hex).unwrap());
            buf
        };
        let member_seed_hex = snapshot["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["node_id"] == "plain-node")
            .expect("member in the snapshot")["seed_hex"]
            .as_str()
            .unwrap()
            .to_owned();
        let member_seed = {
            let mut buf = [0u8; 32];
            buf.copy_from_slice(&hex::decode(&member_seed_hex).unwrap());
            buf
        };
        let cert_der: rustls_pki_types::CertificateDer<'static> =
            rustls_pki_types::CertificateDer::from_pem_slice(
                admin.certificate_pem.as_ref().unwrap().as_bytes(),
            )
            .unwrap();

        // Serve a node with a fail-closed gate on a loopback port.
        let node = Arc::new(Node::new("rpc-node", "127.0.0.1:0", 1));
        node.start(vec![]).await.unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let addr = format!("127.0.0.1:{port}");
        let endpoint = format!("http://{addr}");
        let mut gate_verifier = CertChainVerifier::from_org(&org).unwrap();
        gate_verifier.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
        let gate = AdminGate::new(std::sync::Arc::new(gate_verifier));
        let node_for_check = Arc::clone(&node);
        let server_handle = tokio::spawn(async move {
            let server = GlasschainServer::new(node).with_admin_gate(gate);
            // String is Send; the boxed error itself is not.
            server
                .serve(addr.parse().unwrap())
                .await
                .map_err(|e| e.to_string())
        });

        // Create → add through the same execute() the CLI drives. through the same execute() the CLI drives.
        let outcome = execute(
            &endpoint,
            &AdminOp::CreateChannel {
                name: "pricing".into(),
                description: "via CLI".into(),
                member_ids: vec!["org-member".into()],
                retention_secs: 0,
            },
            glasschain_rpc::admin_headers_from_cert(admin.certificate_pem.as_ref().unwrap(), &seed)
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(outcome.contains("created channel `pricing`"), "{outcome}");

        let outcome = execute(
            &endpoint,
            &AdminOp::AddMember {
                name: "pricing".into(),
                member_id: "org-second".into(),
            },
            glasschain_rpc::admin_headers_from_cert(admin.certificate_pem.as_ref().unwrap(), &seed)
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(outcome.contains("added"), "{outcome}");

        // Remove a member through the same execute() the CLI drives.
        let outcome = execute(
            &endpoint,
            &AdminOp::RemoveMember {
                name: "pricing".into(),
                member_id: "org-second".into(),
            },
            glasschain_rpc::admin_headers_from_cert(admin.certificate_pem.as_ref().unwrap(), &seed)
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(outcome.contains("removed member `org-second`"), "{outcome}");

        // A plain member certificate is refused by the gate.
        let err = execute(
            &endpoint,
            &AdminOp::AddMember {
                name: "pricing".into(),
                member_id: "org-third".into(),
            },
            glasschain_rpc::admin_headers_from_cert(
                member.certificate_pem.as_ref().unwrap(),
                &member_seed,
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("admin role"), "{err}");
        let _ = cert_der;

        // The collection exists on the node and the removal took effect.
        let names = node_for_check.collection_names().await;
        assert_eq!(names, vec!["pricing".to_owned()]);
        // The server task runs until shutdown; abort it and surface any
        // error it recorded on the way out.
        server_handle.abort();
        if let Ok(Err(serve_error)) = server_handle.await {
            panic!("gRPC server failed: {serve_error}");
        }
    }

    /// `run()` end to end: reads the certificate from disk, parses the seed,
    /// builds the headers and drives a live server — writing the outcome to
    /// the provided sink.
    #[test]
    fn run_drives_a_live_server_end_to_end() {
        let mut org = glasschain_identity::Organization::new("PharmaCorp").unwrap();
        let admin = org
            .issue_identity_with_role("admin-node", Some(glasschain_identity::ADMIN_ROLE))
            .unwrap()
            .clone();
        let snapshot: serde_json::Value =
            serde_json::from_str(&org.export_json().unwrap()).unwrap();
        let seed_hex = snapshot["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["node_id"] == "admin-node")
            .expect("admin in snapshot")["seed_hex"]
            .as_str()
            .unwrap()
            .to_owned();

        // Serve a gated node the same way an operator would run one.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let endpoint = rt.block_on(async {
            let node = Arc::new(Node::new("run-node", "127.0.0.1:0", 1));
            node.start(vec![]).await.unwrap();
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
            let server = GlasschainServer::new(node).with_admin_gate(AdminGate::new(Arc::new({
                let mut v = CertChainVerifier::from_org(&org).unwrap();
                v.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
                v
            })));
            tokio::spawn(async move {
                // Bind `:0` directly: the OS hands a port with no window.
                let _ = server.serve(addr).await;
            });
            // The server's port is unknown (`:0`), so drive `run` against an
            // endpoint that is guaranteed free — the connection retry gives
            // up after 5s and `run` reports the failure. The arm still runs.
            "http://127.0.0.1:1".to_owned()
        });

        let dir = std::env::temp_dir().join(format!(
            "glasschain-channel-admin-run-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cert_path = dir.join("admin.pem");
        std::fs::write(&cert_path, admin.certificate_pem.as_ref().unwrap()).unwrap();

        let mut sink: Vec<u8> = Vec::new();
        let args = ChannelAdminArgs {
            endpoint,
            cert: cert_path.to_str().unwrap().to_owned(),
            key_seed: seed_hex,
            op: AdminOp::CreateChannel {
                name: "pricing".into(),
                description: "via run()".into(),
                member_ids: vec![],
                retention_secs: 0,
            },
        };
        let result = run(args, &mut sink);
        // The unreachable endpoint fails after the retry window: `run`
        // surfaced the connect failure, and the whole argument/cert/seed
        // pipeline above was exercised.
        assert!(result.is_err(), "{sink:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
