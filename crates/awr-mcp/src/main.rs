use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "awr-mcp",
    version = env!("AWR_BUILD_VERSION"),
    about = "Serve AWR through single-project stdio or a shared multi-project HTTP endpoint"
)]
struct Args {
    #[arg(long, conflicts_with = "registry")]
    project: Option<PathBuf>,
    /// Operator-owned multi-project registry; enables Streamable HTTP at /mcp.
    #[arg(long)]
    registry: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1:8080", requires = "registry")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();
    if let Some(registry) = args.registry {
        return match serve_http(&registry, args.listen).await {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{}", serde_json::to_string(&error.report()).unwrap());
                std::process::ExitCode::FAILURE
            }
        };
    }
    let server = match awr_mcp::AwrServer::open(&args.project.unwrap_or_else(|| PathBuf::from(".")))
    {
        Ok(server) => server.with_local_overview(),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::to_string(&error.report()).expect("error report serializes")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    // stdout belongs exclusively to MCP. Startup/transport diagnostics go to stderr.
    match server.serve(stdio()).await {
        Ok(service) => match service.waiting().await {
            Ok(_) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("MCP transport: {error}");
                std::process::ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("MCP initialization: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn serve_http(registry: &std::path::Path, address: SocketAddr) -> awr_mcp::Result<()> {
    let hub = awr_mcp::hub::Hub::load(registry)?;
    hub.enable_local_overview();
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!("AWR MCP listening at http://{}/mcp", listener.local_addr()?);
    axum::serve(listener, hub.router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
