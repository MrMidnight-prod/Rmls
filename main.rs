mod install;
mod java;
mod launch;
mod model;
mod mods;
mod tier;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use install::Ctx;
use tier::Tier;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rmcl", version, about = "Minimal Minecraft Java launcher in Rust")]
struct Cli {
    /// Data directory (default: <user data dir>/rmcl)
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args)]
struct Common {
    /// Minecraft version, e.g. 1.21.1, or "latest"
    version: String,
    /// Use the Fabric loader
    #[arg(long)]
    fabric: bool,
    /// Specific Fabric loader version (default: latest stable)
    #[arg(long)]
    loader: Option<String>,
    /// Install performance mods (needs --fabric)
    #[arg(long)]
    perf_mods: bool,
    /// Instance (game directory) name, default: version id
    #[arg(long)]
    instance: Option<String>,
    /// Hardware profile: lowend (default) or normal
    #[arg(long, value_enum, default_value_t = Tier::Lowend)]
    tier: Tier,
    /// Extra Modrinth mod slug to install with --perf-mods (repeatable), e.g. --extra-mod c2me-fabric
    #[arg(long = "extra-mod")]
    extra_mods: Vec<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List available Minecraft versions
    List {
        #[arg(long)]
        snapshots: bool,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Download a version (client, libraries, assets) without launching
    Install(Common),
    /// Install if needed, then launch (singleplayer, or join a server with --server)
    Launch {
        #[command(flatten)]
        common: Common,
        /// Player name (offline mode)
        #[arg(short, long, default_value = "Player")]
        name: String,
        /// Join a server on start, e.g. play.example.com or 127.0.0.1:25565
        #[arg(short, long)]
        server: Option<String>,
        /// Max heap, e.g. 2560M (default depends on --tier)
        #[arg(long)]
        ram: Option<String>,
        /// Path to java; the default "java" = auto-download a runtime on Windows
        #[arg(long, default_value = "java")]
        java: String,
        /// Use generational ZGC instead of tuned G1 (Java 21+)
        #[arg(long)]
        zgc: bool,
        /// Extra JVM argument (repeatable)
        #[arg(long = "jvm-arg", allow_hyphen_values = true)]
        jvm_args: Vec<String>,
        /// Print the command instead of running it
        #[arg(long)]
        dry_run: bool,
    },
}

/// Installs everything and returns (profile, mc_version, instance_dir).
async fn setup(ctx: &Ctx, c: &Common) -> Result<(install::Profile, String, PathBuf)> {
    let mc = install::resolve_version(ctx, &c.version).await?;
    let id = if c.fabric {
        match (&c.loader, install::find_installed_fabric(ctx, &mc)) {
            (None, Some(existing)) => existing,
            _ => install::install_fabric(ctx, &mc, c.loader.as_deref()).await?,
        }
    } else {
        mc.clone()
    };
    let profile = install::prepare(ctx, &id).await?;

    let default_name = if c.fabric { format!("{mc}-fabric") } else { mc.clone() };
    let game_dir = ctx.instances().join(c.instance.clone().unwrap_or(default_name));

    if c.perf_mods {
        if !c.fabric {
            anyhow::bail!("--perf-mods requires --fabric");
        }
        mods::install_perf(ctx, &mc, &game_dir.join("mods"), c.tier, &c.extra_mods).await?;
    }
    Ok((profile, mc, game_dir))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli
        .dir
        .or_else(|| dirs::data_dir().map(|d| d.join("rmcl")))
        .unwrap_or_else(|| PathBuf::from("rmcl-data"));
    std::fs::create_dir_all(&root)?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("rmcl/", env!("CARGO_PKG_VERSION"), " (cli launcher)"))
        .build()?;
    let ctx = Ctx { root, http };

    match cli.cmd {
        Cmd::List { snapshots, limit } => {
            let m = install::fetch_manifest(&ctx).await?;
            println!("latest release: {}   latest snapshot: {}", m.latest.release, m.latest.snapshot);
            for v in m.versions.iter().filter(|v| snapshots || v.kind == "release").take(limit) {
                println!("{:<20} {}", v.id, v.kind);
            }
        }
        Cmd::Install(c) => {
            let (p, _, dir) = setup(&ctx, &c).await?;
            println!("Installed '{}'. Game dir: {}", p.id, dir.display());
        }
        Cmd::Launch { common, name, server, ram, java, zgc, jvm_args, dry_run } => {
            let (p, _, game_dir) = setup(&ctx, &common).await?;
            let java = if java == "java" {
                java::ensure(&ctx, p.json.java_version.as_ref().map(|j| j.major_version).unwrap_or(8)).await?
            } else {
                java
            };
            let ram = ram.unwrap_or_else(|| common.tier.default_ram().to_string());
            launch::run(
                &ctx,
                &p,
                &launch::Opts { name, server, ram, java, zgc, tier: common.tier, dry_run, extra_jvm: jvm_args, game_dir },
            )?;
        }
    }
    Ok(())
}
