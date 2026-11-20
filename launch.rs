use crate::install::{classpath_entries, Ctx, Profile};
use crate::model::*;
use crate::tier::{write_lowend_options, Tier};
use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

pub struct Opts {
    pub name: String,
    pub server: Option<String>,
    pub ram: String,
    pub java: String,
    pub zgc: bool,
    pub tier: Tier,
    pub dry_run: bool,
    pub extra_jvm: Vec<String>,
    pub game_dir: PathBuf,
}

/// Same UUID the vanilla server derives for offline-mode players.
fn offline_uuid(name: &str) -> String {
    let mut h = Md5::new();
    h.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut b: [u8; 16] = h.finalize().into();
    b[6] = (b[6] & 0x0f) | 0x30;
    b[8] = (b[8] & 0x3f) | 0x80;
    let s = hex::encode(b);
    format!("{}-{}-{}-{}-{}", &s[0..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

fn split_host_port(s: &str) -> (String, u16) {
    if let Some((h, p)) = s.rsplit_once(':') {
        if let Ok(p) = p.parse::<u16>() {
            return (h.to_string(), p);
        }
    }
    (s.to_string(), 25565)
}

fn subst(s: &str, vars: &HashMap<String, String>) -> String {
    let mut out = s.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
    }
    out
}

fn expand(args: &[Arg], feats: &HashMap<String, bool>, vars: &HashMap<String, String>) -> Vec<String> {
    let mut out = vec![];
    for a in args {
        match a {
            Arg::Plain(s) => out.push(subst(s, vars)),
            Arg::Ruled { rules, value } => {
                if !rules_allow(rules, feats) {
                    continue;
                }
                match value {
                    StrOrVec::One(s) => out.push(subst(s, vars)),
                    StrOrVec::Many(v) => out.extend(v.iter().map(|s| subst(s, vars))),
                }
            }
        }
    }
    out
}

/// JVM flags per hardware tier.
/// Lowend: small G1 heap, GC thread counts sized for a 2-core/4-thread CPU
/// (logical/2 ~ physical cores), so GC does not steal time from the game threads.
fn tuned_jvm_flags(zgc: bool, tier: Tier) -> Vec<String> {
    let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let (par, conc) = match tier {
        Tier::Lowend => ((logical / 2).max(1), 1),
        Tier::Normal => (logical, (logical / 4).max(1)),
    };
    let mut f: Vec<String> = if zgc {
        vec!["-XX:+UseZGC".into(), "-XX:+ZGenerational".into()]
    } else if tier == Tier::Lowend {
        [
            "-XX:+UseG1GC",
            "-XX:MaxGCPauseMillis=50",
            "-XX:G1HeapRegionSize=4M",
            "-XX:InitiatingHeapOccupancyPercent=30",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    } else {
        [
            "-XX:+UseG1GC",
            "-XX:+ParallelRefProcEnabled",
            "-XX:MaxGCPauseMillis=200",
            "-XX:+UnlockExperimentalVMOptions",
            "-XX:G1NewSizePercent=30",
            "-XX:G1MaxNewSizePercent=40",
            "-XX:G1HeapRegionSize=8M",
            "-XX:G1ReservePercent=20",
            "-XX:G1HeapWastePercent=5",
            "-XX:G1MixedGCCountTarget=4",
            "-XX:InitiatingHeapOccupancyPercent=15",
            "-XX:G1MixedGCLiveThresholdPercent=90",
            "-XX:G1RSetUpdatingPauseTimePercent=5",
            "-XX:SurvivorRatio=32",
            "-XX:MaxTenuringThreshold=1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    f.push("-XX:+DisableExplicitGC".into());
    f.push("-XX:+PerfDisableSharedMem".into());
    f.push(format!("-XX:ParallelGCThreads={par}"));
    f.push(format!("-XX:ConcGCThreads={conc}"));
    f
}

/// (major version, is 64-bit)
fn java_info(java: &str) -> Option<(u32, bool)> {
    let out = Command::new(java).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    let q = text.split('"').nth(1)?;
    let mut parts = q.split('.');
    let first: u32 = parts.next()?.parse().ok()?;
    let major = if first == 1 { parts.next()?.parse().ok()? } else { first };
    Some((major, text.contains("64-Bit")))
}

pub fn run(ctx: &Ctx, p: &Profile, o: &Opts) -> Result<()> {
    let main_class = p.json.main_class.clone().context("profile has no mainClass")?;

    let req = p.json.java_version.as_ref().map(|j| j.major_version).unwrap_or(0);
    match java_info(&o.java) {
        Some((have, is64)) => {
            if have < req {
                bail!("This version needs Java {req}+, but '{}' is Java {have}. Use --java <path>.", o.java);
            }
            if !is64 {
                eprintln!("warning: 32-bit Java detected - it cannot use more than ~1.5 GB. Install a 64-bit JDK/JRE.");
            }
        }
        None => eprintln!("warning: could not detect Java version of '{}' (need {req}+)", o.java),
    }
    if o.zgc && o.tier == Tier::Lowend {
        eprintln!("warning: ZGC is a poor fit for a 2-core CPU; the default G1 profile is recommended.");
    }

    std::fs::create_dir_all(&o.game_dir)?;
    let sep = if cfg!(windows) { ";" } else { ":" };
    let classpath = classpath_entries(ctx, p)
        .iter()
        .map(|x| x.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(sep);

    let natives = ctx.versions().join(&p.id).join("natives");
    let assets_id = p
        .json
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .or_else(|| p.json.assets.clone())
        .unwrap_or_else(|| "legacy".into());

    let mut vars: HashMap<String, String> = HashMap::new();
    let mut set = |k: &str, v: String| {
        vars.insert(k.to_string(), v);
    };
    set("auth_player_name", o.name.clone());
    set("auth_uuid", offline_uuid(&o.name));
    set("auth_access_token", "0".into());
    set("auth_session", "0".into());
    set("auth_xuid", "0".into());
    set("clientid", "0".into());
    set("user_type", "legacy".into());
    set("user_properties", "{}".into());
    set("version_name", p.id.clone());
    set("version_type", p.json.kind.clone().unwrap_or_else(|| "release".into()));
    set("game_directory", o.game_dir.to_string_lossy().to_string());
    set("assets_root", ctx.assets().to_string_lossy().to_string());
    set("game_assets", ctx.assets().to_string_lossy().to_string());
    set("assets_index_name", assets_id);
    set("natives_directory", natives.to_string_lossy().to_string());
    set("library_directory", ctx.libraries().to_string_lossy().to_string());
    set("classpath_separator", sep.into());
    set("classpath", classpath);
    set("launcher_name", "rmcl".into());
    set("launcher_version", env!("CARGO_PKG_VERSION").into());
    set("resolution_width", "854".into());
    set("resolution_height", "480".into());

    let mut feats: HashMap<String, bool> = HashMap::new();
    if let Some(s) = &o.server {
        feats.insert("is_quick_play_multiplayer".into(), true);
        vars.insert("quickPlayMultiplayer".into(), s.clone());
    }

    let (mut jvm, mut game) = match (&p.json.arguments, &p.json.minecraft_arguments) {
        (Some(a), _) => (expand(&a.jvm, &feats, &vars), expand(&a.game, &feats, &vars)),
        (None, Some(legacy)) => {
            let jvm_t = [
                "-Djava.library.path=${natives_directory}",
                "-Dminecraft.launcher.brand=${launcher_name}",
                "-Dminecraft.launcher.version=${launcher_version}",
                "-cp",
                "${classpath}",
            ];
            (
                jvm_t.iter().map(|s| subst(s, &vars)).collect(),
                legacy.split_whitespace().map(|s| subst(s, &vars)).collect(),
            )
        }
        _ => bail!("profile has no arguments"),
    };

    // Versions without quick-play support (< 1.20): classic --server/--port flags.
    if let Some(s) = &o.server {
        if !game.iter().any(|a| a == s || a == "--quickPlayMultiplayer") {
            let (h, port) = split_host_port(s);
            game.extend(["--server".into(), h, "--port".into(), port.to_string()]);
        }
    }

    let mut cmd_args: Vec<String> = tuned_jvm_flags(o.zgc, o.tier);
    cmd_args.push(format!("-Xmx{}", o.ram));
    cmd_args.push("-Xms1G".into());
    cmd_args.append(&mut o.extra_jvm.clone());
    cmd_args.append(&mut jvm);
    cmd_args.push(main_class);
    cmd_args.append(&mut game);

    if o.dry_run {
        let q: Vec<String> = cmd_args
            .iter()
            .map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() })
            .collect();
        println!("{} {}", o.java, q.join(" "));
        return Ok(());
    }

    if o.tier == Tier::Lowend {
        write_lowend_options(&o.game_dir)?;
    }
    println!("Launching {} as '{}' (game dir: {})", p.id, o.name, o.game_dir.display());
    let status = Command::new(&o.java)
        .args(&cmd_args)
        .current_dir(&o.game_dir)
        .status()
        .with_context(|| format!("failed to start '{}'", o.java))?;
    if !status.success() {
        bail!("game exited with {status}");
    }
    Ok(())
}
