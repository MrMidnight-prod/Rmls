# rmcl — minimal Minecraft Java launcher in Rust (CLI)

Build: `cargo build --release` -> `target/release/rmcl` (CLI) and `target/release/rmcl-gui` (GUI, keep both files in the same folder).

GUI: run `rmcl-gui`, pick version / profile, press **Play**. First run downloads everything (watch the log pane).
CLI profile flag: `--tier lowend` (default, ~2.5 GB heap, light options.txt, light mod set) or `--tier normal`.

Windows installer: push this folder to a GitHub repo -> Actions -> "build-windows-installer" -> Run workflow ->
download artifact `rmcl-setup` (contains rmcl-setup.exe). Java is downloaded automatically on first launch.

Tips for GT 720M + Optimus laptops: in NVIDIA Control Panel ->
Manage 3D settings -> Program settings, add `java.exe` from `%APPDATA%\\rmcl\\runtime\\java17\\bin` and set "High-performance NVIDIA processor".


```
rmcl list [--snapshots] [--limit 30]
rmcl install 1.21.1 [--fabric] [--perf-mods]
rmcl launch  1.21.1 --name Steve                      # singleplayer
rmcl launch  1.21.1 --name Steve --server mc.example.com:25565
rmcl launch  latest --fabric --perf-mods --ram 6G     # Fabric + multithreaded optimisation mods
rmcl launch  1.21.1 --dry-run                         # print the java command only
```

Data lives in `<user data dir>/rmcl` (override with `--dir`):
`versions/ libraries/ assets/ instances/<name>/` (saves, mods, options live in the instance).

## Limitations (v0.1)
- Offline auth only (no Microsoft login yet): fine for singleplayer and offline-mode servers.
- Pre-1.6 "legacy/virtual" assets are not handled.
- Existing files are trusted (SHA1 is checked only on download).
- Not yet compiled/tested by the author: expect to fix a few compiler errors.
