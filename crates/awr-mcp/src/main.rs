use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "awr-mcp",
    version = env!("AWR_BUILD_VERSION"),
    about = "Serve AWR through single-project stdio, the local project catalog over stdio, or a shared multi-project HTTP endpoint"
)]
struct Args {
    /// With --catalog, the default project; otherwise the one project served.
    #[arg(long, conflicts_with = "registry")]
    project: Option<PathBuf>,
    /// Serve the registered projects in the per-user local catalog over stdio.
    #[arg(long, conflicts_with = "registry")]
    catalog: bool,
    /// With --catalog, serve only projects under this directory; repeatable.
    #[arg(long = "allow-root", requires = "catalog")]
    allow_root: Vec<PathBuf>,
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
    let opened = if args.catalog {
        // The default project is --project, else the starting directory when it is one.
        let default = args.project.or_else(|| {
            let here = PathBuf::from(".");
            here.join(".awr/project.toml").is_file().then_some(here)
        });
        awr_mcp::AwrServer::open_catalog(&args.allow_root, default.as_deref())
    } else {
        awr_mcp::AwrServer::open(&args.project.unwrap_or_else(|| PathBuf::from(".")))
    };
    let server = match opened {
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
