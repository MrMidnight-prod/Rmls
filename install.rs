use crate::model::*;
use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const PARALLEL_LIBS: usize = 16;
const PARALLEL_ASSETS: usize = 32;

#[derive(Clone)]
pub struct Ctx {
    pub root: PathBuf,
    pub http: Client,
}

impl Ctx {
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }
    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }
}

pub struct Profile {
    pub id: String,
    pub json: VersionJson,
    pub client_jar: PathBuf,
}

pub fn read_json<T: DeserializeOwned>(p: &Path) -> Result<T> {
    let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", p.display()))
}

// ---------- downloading ----------

async fn fetch(http: &Client, url: &str) -> Result<Vec<u8>> {
    Ok(http.get(url).send().await?.error_for_status()?.bytes().await?.to_vec())
}

/// Downloads `url` to `dest` (skips if the file already exists), verifies SHA1, 3 retries.
pub async fn download(http: &Client, url: &str, dest: &Path, sha1: Option<&str>) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    let mut last: Option<anyhow::Error> = None;
    for _ in 0..3 {
        match fetch(http, url).await {
            Ok(bytes) => {
                if let Some(exp) = sha1 {
                    let got = hex::encode(Sha1::digest(&bytes));
                    if !got.eq_ignore_ascii_case(exp) {
                        last = Some(anyhow::anyhow!("SHA1 mismatch (got {got}, expected {exp})"));
                        continue;
                    }
                }
                let mut tmp = dest.as_os_str().to_owned();
                tmp.push(".part");
                let tmp = PathBuf::from(tmp);
                tokio::fs::write(&tmp, &bytes).await?;
                tokio::fs::rename(&tmp, dest).await?;
                return Ok(());
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap().context(format!("downloading {url}")))
}

// ---------- version resolution ----------

pub async fn fetch_manifest(ctx: &Ctx) -> Result<Manifest> {
    Ok(ctx.http.get(MANIFEST_URL).send().await?.error_for_status()?.json().await?)
}

pub async fn resolve_version(ctx: &Ctx, v: &str) -> Result<String> {
    if v == "latest" || v == "latest-snapshot" {
        let m = fetch_manifest(ctx).await?;
        return Ok(if v == "latest" { m.latest.release } else { m.latest.snapshot });
    }
    Ok(v.to_string())
}

pub async fn install_vanilla(ctx: &Ctx, id: &str) -> Result<()> {
    let dir = ctx.versions().join(id);
    let json_path = dir.join(format!("{id}.json"));
    if !json_path.exists() {
        let m = fetch_manifest(ctx).await?;
        let e = m
            .versions
            .iter()
            .find(|v| v.id == id)
            .with_context(|| format!("version '{id}' not found in Mojang manifest"))?;
        download(&ctx.http, &e.url, &json_path, None).await?;
    }
    let vj: VersionJson = read_json(&json_path)?;
    if let Some(c) = vj.downloads.as_ref().and_then(|d| d.client.as_ref()) {
        download(&ctx.http, &c.url, &dir.join(format!("{id}.jar")), c.sha1.as_deref()).await?;
    }
    Ok(())
}

pub fn load_profile(ctx: &Ctx, id: &str) -> Result<Profile> {
    let child: VersionJson = read_json(&ctx.versions().join(id).join(format!("{id}.json")))?;
    match child.inherits_from.clone() {
        Some(pid) => {
            let parent: VersionJson =
                read_json(&ctx.versions().join(&pid).join(format!("{pid}.json")))?;
            Ok(Profile {
                id: id.to_string(),
                json: merge(parent, child),
                client_jar: ctx.versions().join(&pid).join(format!("{pid}.jar")),
            })
        }
        None => Ok(Profile {
            id: id.to_string(),
            json: child,
            client_jar: ctx.versions().join(id).join(format!("{id}.jar")),
        }),
    }
}

/// Make sure everything needed to launch `id` is on disk and return the merged profile.
pub async fn prepare(ctx: &Ctx, id: &str) -> Result<Profile> {
    let json_path = ctx.versions().join(id).join(format!("{id}.json"));
    let base = if json_path.exists() {
        let v: VersionJson = read_json(&json_path)?;
        v.inherits_from.unwrap_or_else(|| id.to_string())
    } else {
        id.to_string()
    };
    install_vanilla(ctx, &base).await?;
    if base != id && !json_path.exists() {
        anyhow::bail!("profile '{id}' is missing");
    }
    let profile = load_profile(ctx, id)?;
    install_libraries(ctx, &profile).await?;
    install_assets(ctx, &profile).await?;
    Ok(profile)
}

// ---------- libraries ----------

pub struct LibEntry {
    pub rel: String,
    pub url: String,
    pub sha1: Option<String>,
    /// Some(..) => this is a legacy natives jar that must be extracted (not put on the classpath)
    pub native_exclude: Option<Vec<String>>,
}

pub fn maven_path(name: &str) -> Option<String> {
    let (coords, ext) = name.split_once('@').unwrap_or((name, "jar"));
    let p: Vec<&str> = coords.split(':').collect();
    if p.len() < 3 {
        return None;
    }
    let g = p[0].replace('.', "/");
    let file = if p.len() >= 4 {
        format!("{}-{}-{}.{}", p[1], p[2], p[3], ext)
    } else {
        format!("{}-{}.{}", p[1], p[2], ext)
    };
    Some(format!("{g}/{}/{}/{file}", p[1], p[2]))
}

pub fn lib_entries(lib: &Library) -> Vec<LibEntry> {
    let no_features = HashMap::new();
    if let Some(r) = &lib.rules {
        if !rules_allow(r, &no_features) {
            return vec![];
        }
    }
    let mut out = vec![];
    let dl = lib.downloads.as_ref();

    if let Some(a) = dl.and_then(|d| d.artifact.as_ref()) {
        if let Some(rel) = a.path.clone().or_else(|| maven_path(&lib.name)) {
            out.push(LibEntry { rel, url: a.url.clone(), sha1: a.sha1.clone(), native_exclude: None });
        }
    } else if dl.is_none() {
        // Fabric-style library: only "name" + maven "url"
        if let Some(rel) = maven_path(&lib.name) {
            let mut base = lib.url.clone().unwrap_or_else(|| "https://libraries.minecraft.net/".into());
            if !base.ends_with('/') {
                base.push('/');
            }
            out.push(LibEntry { url: format!("{base}{rel}"), rel, sha1: lib.sha1.clone(), native_exclude: None });
        }
    }

    // Legacy (<= 1.18) natives classifiers
    if let (Some(n), Some(d)) = (&lib.natives, dl) {
        if let Some(cls) = n.get(os_name()) {
            let cls = cls.replace("${arch}", if cfg!(target_pointer_width = "64") { "64" } else { "32" });
            if let Some(a) = d.classifiers.as_ref().and_then(|c| c.get(&cls)) {
                if let Some(rel) = a.path.clone().or_else(|| maven_path(&lib.name)) {
                    let mut ex = lib.extract.clone().unwrap_or_default().exclude;
                    ex.push("META-INF/".into());
                    out.push(LibEntry { rel, url: a.url.clone(), sha1: a.sha1.clone(), native_exclude: Some(ex) });
                }
            }
        }
    }
    out
}

pub fn classpath_entries(ctx: &Ctx, p: &Profile) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut v = vec![];
    for e in p.json.libraries.iter().flat_map(lib_entries) {
        if e.native_exclude.is_none() && seen.insert(e.rel.clone()) {
            v.push(ctx.libraries().join(&e.rel));
        }
    }
    v.push(p.client_jar.clone());
    v
}

pub async fn install_libraries(ctx: &Ctx, p: &Profile) -> Result<()> {
    let entries: Vec<LibEntry> = p.json.libraries.iter().flat_map(lib_entries).collect();
    println!("Libraries: {}", entries.len());

    let tasks: Vec<_> = entries
        .iter()
        .map(|e| {
            let http = ctx.http.clone();
            let dest = ctx.libraries().join(&e.rel);
            let url = e.url.clone();
            let sha = e.sha1.clone();
            async move { download(&http, &url, &dest, sha.as_deref()).await }
        })
        .collect();
    let results: Vec<Result<()>> = stream::iter(tasks).buffer_unordered(PARALLEL_LIBS).collect().await;
    for r in results {
        r?;
    }

    let natives_dir = ctx.versions().join(&p.id).join("natives");
    for e in entries.iter().filter(|e| e.native_exclude.is_some()) {
        extract_natives(&ctx.libraries().join(&e.rel), &natives_dir, e.native_exclude.as_deref().unwrap())?;
    }
    std::fs::create_dir_all(&natives_dir)?;
    Ok(())
}

fn extract_natives(jar: &Path, dest: &Path, exclude: &[String]) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let mut z = zip::ZipArchive::new(std::fs::File::open(jar)?)?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        if exclude.iter().any(|x| name.starts_with(x.as_str())) {
            continue;
        }
        let Some(rel) = f.enclosed_name() else { continue };
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut o = std::fs::File::create(out)?;
        std::io::copy(&mut f, &mut o)?;
    }
    Ok(())
}

// ---------- assets ----------

#[derive(Deserialize)]
struct AssetIndex {
    objects: HashMap<String, AssetObj>,
}
#[derive(Deserialize)]
struct AssetObj {
    hash: String,
}

pub async fn install_assets(ctx: &Ctx, p: &Profile) -> Result<()> {
    let Some(ai) = &p.json.asset_index else { return Ok(()) };
    let idx_path = ctx.assets().join("indexes").join(format!("{}.json", ai.id));
    download(&ctx.http, &ai.url, &idx_path, ai.sha1.as_deref()).await?;
    let idx: AssetIndex = read_json(&idx_path)?;

    let uniq: HashSet<&String> = idx.objects.values().map(|o| &o.hash).collect();
    let missing: Vec<&String> = uniq
        .into_iter()
        .filter(|h| !ctx.assets().join("objects").join(&h[..2]).join(h.as_str()).exists())
        .collect();
    println!("Assets: {} missing", missing.len());

    let tasks: Vec<_> = missing
        .into_iter()
        .map(|h| {
            let http = ctx.http.clone();
            let dest = ctx.assets().join("objects").join(&h[..2]).join(h);
            let url = format!("https://resources.download.minecraft.net/{}/{}", &h[..2], h);
            let sha = h.clone();
            async move { download(&http, &url, &dest, Some(&sha)).await }
        })
        .collect();
    let results: Vec<Result<()>> = stream::iter(tasks).buffer_unordered(PARALLEL_ASSETS).collect().await;
    for r in results {
        r?;
    }
    Ok(())
}

// ---------- Fabric ----------

pub async fn install_fabric(ctx: &Ctx, mc: &str, loader: Option<&str>) -> Result<String> {
    #[derive(Deserialize)]
    struct E {
        loader: L,
    }
    #[derive(Deserialize)]
    struct L {
        version: String,
        stable: bool,
    }

    let loader = match loader {
        Some(l) => l.to_string(),
        None => {
            let v: Vec<E> = ctx
                .http
                .get(format!("https://meta.fabricmc.net/v2/versions/loader/{mc}"))
                .send()
                .await?
                .error_for_status()
                .context("Fabric does not support this Minecraft version")?
                .json()
                .await?;
            v.iter()
                .find(|e| e.loader.stable)
                .or(v.first())
                .context("no Fabric loader versions found")?
                .loader
                .version
                .clone()
        }
    };

    let url = format!("https://meta.fabricmc.net/v2/versions/loader/{mc}/{loader}/profile/json");
    let text = ctx.http.get(&url).send().await?.error_for_status()?.text().await?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let id = v["id"].as_str().context("Fabric profile has no id")?.to_string();
    let dir = ctx.versions().join(&id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{id}.json")), text)?;
    println!("Fabric profile: {id}");
    Ok(id)
}

/// Newest already-installed Fabric profile for this Minecraft version, if any.
pub fn find_installed_fabric(ctx: &Ctx, mc: &str) -> Option<String> {
    let suffix = format!("-{mc}");
    let mut found: Vec<String> = std::fs::read_dir(ctx.versions())
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("fabric-loader-") && n.ends_with(&suffix))
        .collect();
    found.sort();
    found.pop()
}
