use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use local_ip_address::local_ip;
use qrcode::QrCode;

mod auth;
mod pty;
mod server;

#[derive(Parser)]
#[command(name = "remote-terminal", about = "Remote terminal via browser")]
struct Cli {
    /// Port to listen on
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Authentication token (auto-generated if not set)
    #[arg(short, long)]
    token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("remote_terminal=info")
        .init();

    let cli = Cli::parse();

    let token = cli.token.unwrap_or_else(|| {
        use std::fmt::Write;
        let bytes = uuid::Uuid::new_v4();
        let mut s = String::with_capacity(8);
        write!(s, "{}", bytes).unwrap();
        s.truncate(8);
        s
    });

    let state = server::AppState {
        pty_manager: Arc::new(pty::PtyManager::new()),
        token: Arc::new(token.clone()),
    };

    let app = server::build_router(state);

    let ip = local_ip().unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)));
    let url = format!("http://{ip}:{}/?token={token}", cli.port);

    print_banner(&url);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port)).await?;
    tracing::info!("Listening on 0.0.0.0:{}", cli.port);

    axum::serve(listener, app).await?;

    Ok(())
}

fn print_banner(url: &str) {
    let code = QrCode::new(url).unwrap();
    let string = code
        .render::<char>()
        .quiet_zone(false)
        .module_dimensions(2, 1)
        .build();

    println!();
    println!("  ┌──────────────────────────────────────┐");
    println!("  │  Remote Terminal                      │");
    println!("  │  Scan QR code to connect from mobile  │");
    println!("  └──────────────────────────────────────┘");
    println!();
    for line in string.lines() {
        println!("  {line}");
    }
    println!();
    println!("  URL: {url}");
    println!();
}
