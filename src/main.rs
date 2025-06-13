use anyhow::Result;
use clap::Parser;
use tracing::info;

mod p2p_node;

use p2p_node::P2PNode;

#[derive(Parser, Debug)]
#[command(author, version, about = "P2P network node")]
struct Args {
  
    #[arg(short, long)]
    port: Option<u16>,
    
    #[arg(short, long)]
    connect: Option<String>,
    
    #[arg(long, default_value = "true")]
    bootstrap: bool,
    
    #[arg(long, default_value = "false")]
    relay: bool,
    
    #[arg(short, long)]
    name: Option<String>,
    
    #[arg(long, default_value = "true")]
    dht: bool,
    
    #[arg(long, default_value = "true")]
    mdns: bool,
}

#[tokio::main]
async fn main() -> Result<()> {

    tracing_subscriber::fmt()
        .with_env_filter("info,libp2p=debug")
        .init();
    
    let args = Args::parse();
    
    info!("🚀 Starting P2P node...");
    
    let mut node = P2PNode::new(
        args.name, 
        args.port, 
        args.dht, 
        args.mdns, 
        args.bootstrap,
        args.relay
    ).await?;
    
    if let Some(addr) = args.connect {
        node.connect_to_peer(&addr).await?;
    }
    
    node.run().await
}
