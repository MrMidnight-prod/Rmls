use crate::install::{download, Ctx};
use crate::tier::Tier;
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct MrVersion {
    version_type: String,
    files: Vec<MrFile>,
}
#[derive(Deserialize)]
struct MrFile {
    url: String,
    filename: String,
    primary: bool,
    hashes: MrHashes,
}
#[derive(Deserialize)]
struct MrHashes {
    sha1: Option<String>,
}

pub async fn install_perf(ctx: &Ctx, mc: &str, mods_dir: &Path, tier: Tier, extra: &[String]) -> Result<()> {
    std::fs::create_dir_all(mods_dir)?;
    println!("Installing performance mods for {mc} (Modrinth):");
    let slugs: Vec<String> = tier.mods().iter().map(|s| s.to_string()).chain(extra.iter().cloned()).collect();
    for slug in &slugs {
        let resp = ctx
            .http
            .get(format!("https://api.modrinth.com/v2/project/{slug}/version"))
            .query(&[
                ("game_versions", format!("[\"{mc}\"]")),
                ("loaders", "[\"fabric\"]".to_string()),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            eprintln!("  ! {slug}: HTTP {}", resp.status());
            continue;
        }
        let vs: Vec<MrVersion> = resp.json().await?;
        let Some(v) = vs.iter().find(|v| v.version_type == "release").or(vs.first()) else {
            println!("  - {slug}: no build for {mc}, skipped");
            continue;
        };
        let Some(f) = v.files.iter().find(|f| f.primary).or(v.files.first()) else {
            continue;
        };
        download(&ctx.http, &f.url, &mods_dir.join(&f.filename), f.hashes.sha1.as_deref()).await?;
        println!("  + {slug}: {}", f.filename);
    }
    Ok(())
}
