use crate::install::{download, Ctx};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Windows x64: download a portable Temurin JRE once into <data>/runtime/javaN and use it.
/// Other platforms: fall back to "java" from PATH.
pub async fn ensure(ctx: &Ctx, required: u32) -> Result<String> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return Ok("java".into());
    }
    let major = match required {
        0..=8 => 8,
        9..=17 => 17,
        n => n,
    };
    let dir = ctx.root.join("runtime").join(format!("java{major}"));
    let exe = dir.join("bin").join("java.exe");
    if exe.exists() {
        return Ok(exe.to_string_lossy().into());
    }
    println!("Downloading Java {major} runtime (one-time, ~50 MB)...");
    let url = format!("https://api.adoptium.net/v3/binary/latest/{major}/ga/windows/x64/jre/hotspot/normal/eclipse");
    let zip_path = ctx.root.join("runtime").join(format!("java{major}.zip"));
    download(&ctx.http, &url, &zip_path, None).await?;
    extract_strip1(&zip_path, &dir)?;
    let _ = std::fs::remove_file(&zip_path);
    if !exe.exists() {
        bail!("Java runtime unpacked but {} not found", exe.display());
    }
    Ok(exe.to_string_lossy().into())
}

/// Unzip, dropping the archive's single top-level folder.
fn extract_strip1(zip_path: &Path, dest: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let Some(name) = f.enclosed_name() else { continue };
        let rel: PathBuf = name.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::io::copy(&mut f, &mut std::fs::File::create(out)?)?;
        }
    }
    Ok(())
}
