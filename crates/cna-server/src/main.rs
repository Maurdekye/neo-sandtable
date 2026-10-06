use cna_server::{campaigns as factory, http::App};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = std::env::var_os("CNA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../data"));
    let campaigns = std::env::var_os("CNA_CAMPAIGN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../../campaigns"));
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
