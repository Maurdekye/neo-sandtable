use cna_server::{campaigns as factory, http::App};
use std::{io::Write, path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = std::env::var_os("CNA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../data"));
    let campaigns = std::env::var_os("CNA_CAMPAIGN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../../campaigns"));
    // Local operator access only, before opening a listener, migrating a database,
    // recovering a writer, creating directories or minting network credentials.
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        if args.len() != 2 || args[0] != "operator-failures"
            || args[1].is_empty() || args[1].len() > 128
            || !args[1].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("usage: cna-server operator-failures <campaign>".into());
        }
        let records = cna_server::operator_failures::read_campaign(&campaigns.join(format!("{}.sqlite", args[1])), &args[1])
            .map_err(|_| std::io::Error::other("operator failure records unavailable"))?;
        let mut output = Vec::new();
        for record in records {
            let availability = match record.availability {
                cna_server::operator_failures::Availability::Available => "available",
                cna_server::operator_failures::Availability::Missing => "missing",
                cna_server::operator_failures::Availability::Legacy => "legacy",
                cna_server::operator_failures::Availability::Corrupt => "corrupt",
                cna_server::operator_failures::Availability::Mismatched => "mismatched",
            };
            let metadata = serde_json::json!({
                "committed_revision": record.committed_revision,
                "state_hash": record.state_hash,
                "availability": availability,
            });
            serde_json::to_writer(&mut output, &metadata)?;
            output.push(b'\n');
            if let Some(payload) = record.private_payload() {
                output.extend_from_slice(payload.as_bytes());
                output.push(b'\n');
            }
        }
        if output.len() > 2 * 1024 * 1024 {
            return Err("operator failure output unavailable".into());
        }
        std::io::stdout().lock().write_all(&output)?;
        return Ok(());
    }
    let port = std::env::var("CNA_PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse::<u16>()?;
    std::fs::create_dir_all(&campaigns)?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let port = listener.local_addr()?.port();
    let factory_data = data.clone();
    let app = App::new(
        campaigns.clone(),
        port,
        Arc::new(move |request, directory| factory::create(directory, &factory_data, request)),
    );
    let files = std::fs::read_dir(&campaigns)?.collect::<Result<Vec<_>, _>>()?;
    for file in files {
        let path = file.path();
        if path.extension().is_some_and(|e| e == "sqlite") {
            match factory::recover(&path, &data) {
                Ok(handle) => app.register(handle),
                Err(error) => {
                    app.shutdown().await;
                    return Err(error.into());
                }
            }
        }
    }
    let credentials = std::env::var_os("CNA_CAPABILITY_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| campaigns.join("operator-capabilities.json"));
    if let Err(error) = app.write_credentials(&credentials) {
        app.shutdown().await;
        return Err(error.into());
    }
    println!(
        "neo-sandtable operator board http://127.0.0.1:{port}/#cap={}",
        app.operator_token()
    );
    println!(
        "Operator credentials (trusted launcher only): {}",
        credentials.display()
    );
    let shutdown = app.clone();
    let router = app.router(&root.join("../../web/dist"));
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown.shutdown().await;
        })
        .await;
    app.shutdown().await;
    result?;
    Ok(())
}
