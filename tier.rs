use anyhow::Result;
use clap::ValueEnum;
use std::path::Path;

/// Hardware profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Tier {
    /// Old dual-core CPU, weak GPU, ~4-6 GB RAM (default)
    Lowend,
    /// Modern multi-core machine
    Normal,
}

impl Tier {
    pub fn default_ram(self) -> &'static str {
        match self {
            // 6 GB total: leave ~3.5 GB for Windows + the JVM's own overhead
            Tier::Lowend => "2560M",
            Tier::Normal => "4G",
        }
    }

    /// Modrinth slugs. Mods without a build for the chosen version are skipped.
    pub fn mods(self) -> &'static [&'static str] {
        match self {
            // No C2ME: extra worker threads fight the render thread on 2 cores and cost RAM.
            Tier::Lowend => &[
                "fabric-api",
                "sodium",         // rendering (OpenGL 3.3+)
                "lithium",        // game logic / tick
                "ferrite-core",   // less RAM
                "modernfix",      // less RAM, faster load
                "memoryleakfix",  // less RAM
                "entityculling",  // don't render hidden entities
                "immediatelyfast",// cheaper HUD / batching
                "scalablelux",    // faster light engine (skipped where unsupported)
                "noisium",        // faster world generation (1.18+)
                "dynamic-fps",    // throttle when window unfocused
                "sodium-extra",   // extra video options
            ],
            Tier::Normal => &[
                "fabric-api",
                "sodium",
                "lithium",
                "c2me-fabric",
                "scalablelux",
                "ferrite-core",
                "modernfix",
                "immediatelyfast",
            ],
        }
    }
}

/// Light settings for weak GPUs. Written only if the instance has no options.txt yet,
/// so the player's own settings are never overwritten.
pub fn write_lowend_options(game_dir: &Path) -> Result<()> {
    let p = game_dir.join("options.txt");
    if p.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(game_dir)?;
    std::fs::write(
        p,
        "renderDistance:6\n\
         simulationDistance:5\n\
         maxFps:60\n\
         graphicsMode:0\n\
         fancyGraphics:false\n\
         particles:2\n\
         renderClouds:\"false\"\n\
         entityShadows:false\n\
         biomeBlendRadius:1\n\
         mipmapLevels:1\n\
         entityDistanceScaling:0.75\n",
    )?;
    println!("Wrote low-end default options.txt");
    Ok(())
}
