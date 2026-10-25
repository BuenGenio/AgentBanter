# Building and publishing AgentBanter

## What ships

Each release archive contains one executable with embedded templates and static assets, a user installer, a desktop entry, an SVG icon, the project license, and the bundled JavaScript licenses, and Rust dependency notices and license files. Viewing conversations needs no Rust, Node or Python runtime. Linux archives use musl; macOS archives are built separately for Apple Silicon and Intel.

The package defaults to loopback only. Capture and sending are enabled in the user's config or with explicit flags. Packaged builds never discover templates or scripts from the current working directory. Developers can opt into hot reload with `--templates ./templates --static ./static`.

## Local build and install

```sh
cargo test --locked
cargo build --release --locked
cargo install --path . --locked
agentbanter open
```

`agentbanter open` starts a detached server if none is listening on its configured loopback address. It waits for `/healthz` before opening a browser and reuses an existing AgentBanter server. Other applications occupying the port produce an error. `--no-browser` prints the URL for SSH/headless use. `agentbanter --open` is an alias. `agentbanter serve` (or no command) runs in the foreground; Ctrl+C stops it.

Detached server output is appended to the user's local data directory under `agentbanter/logs/server.log`. The file is created with mode 0600 on Unix. Existing servers keep their current settings; restart them after config changes. For managed startup and stop commands, use Homebrew services or the Linux user service rather than an unmanaged detached process.

## Shared config

```sh
agentbanter config init
# Or: agentbanter --config /absolute/path/settings.json config init
```

The default is the OS config directory: `~/.config/agentbanter/config.json` on Linux, `~/Library/Application Support/agentbanter/config.json` on macOS. Existing files are never overwritten. CLI values override matching config settings. Relative data/asset paths in the config resolve relative to that config file; use absolute executable paths for `codex_bin` and `claude_bin` when GUI or service PATH differs from your shell. Boolean flags enable the corresponding feature; disable it by setting the config value to false and restarting the server.

Example:

```json
{
  "bind": ["127.0.0.1"],
  "port": 7331,
  "sources": ["claude", "codex", "chatgpt", "ollama"],
  "allow_capture": true,
  "allow_send": true,
  "allow_ollama": true,
  "ollama_url": "http://127.0.0.1:11434",
  "poll_ms": 750
}
```

Optional keys: `projects_dir`, `codex_dir`, `chatgpt_dir`, `ollama_dir`, `templates`, `static_dir`, `codex_bin`, `claude_bin`, and `send_permission_mode`. No config is required for a normal viewing session. `allow_ollama` defaults to false and requires every bind address to be loopback; it enables chat and recording at `/local` and `/ollama`. The Ollama endpoint must be a loopback HTTP origin. An existing source list need not be edited when enabling Ollama: its recording directory is included automatically. `open` requires the first bind address to be loopback; explicit remote binding is supported by `serve` when local inference and capture are disabled.

## GNOME / KDE

From a source install or copied binary:

```sh
agentbanter desktop install
agentbanter desktop uninstall
```

This writes the application entry under `$XDG_DATA_HOME/applications` and icon under `$XDG_DATA_HOME/icons/hicolor/scalable/apps` (normally `~/.local/share`). The executable path is absolute and quoted, so shell PATH differences and spaces in install paths do not prevent launching. The entry runs `agentbanter open` without a terminal. Reinstall the launcher if you move the binary. Homebrew's Linux formula installs its launcher under the Homebrew share prefix; if the desktop does not discover that prefix, run `agentbanter desktop install` once as your normal user.

For a native Linux user service:

```sh
agentbanter service install
agentbanter service start
agentbanter service status
agentbanter open
agentbanter service stop
# Optional startup at login:
systemctl --user enable agentbanter.service
# Remove:
agentbanter service uninstall
```

The service runs `serve` and reads the same config. A custom `--config` supplied to `service install` is recorded as an absolute path. Installation reloads systemd but does not automatically start or enable the service. A systemd user session is required; the service installer reports errors rather than claiming success when none is available. It never installs a root service. Avoid starting a separate detached server before starting the managed service on the same port. Logs: `journalctl --user -u agentbanter.service`.

On macOS with the published Homebrew formula, use `brew services start agentbanter`, `brew services restart agentbanter`, and `brew services stop agentbanter`. Homebrew manages launchd for the current user. The native `service` command is Linux-only.

## Build an archive

Requires Python 3.9+ on the build machine, not on the installed machine:

```sh
python3 scripts/package.py archive \
  --binary target/release/agentbanter \
  --target aarch64-apple-darwin
```

The target must match the executable you built. Archives and per-file SHA-256 checksums go into `dist/`. Extract one and run `./install.sh`; it defaults to `~/.local/bin`, installs license files, and registers the Linux launcher. `--prefix /absolute/path` changes the install prefix; `--no-desktop` skips desktop registration. Add that prefix's `bin` directory to PATH if necessary. The installer does not request sudo or start an agent session.

## CI and release

`.github/workflows/check.yml` runs Rust, decoder/composer and packaging tests plus the standalone-binary smoke test. `.github/workflows/release.yml` builds these targets natively:

- `aarch64-apple-darwin` on `macos-14`.
- `x86_64-apple-darwin` on `macos-15-intel`.
- `x86_64-unknown-linux-musl` on `ubuntu-24.04`.
- `aarch64-unknown-linux-musl` on `ubuntu-24.04-arm`.

The Linux jobs validate the desktop entry and its installation in an isolated home directory. Browser launchers are mocked; smoke tests send no prompts to agents. The release assembly job requires all four archives, derives each formula checksum from the bytes, checks Ruby syntax, and produces `SHA256SUMS` and `agentbanter.rb`. Manual workflow dispatch creates build artifacts by default. With `create_draft: true`, it creates a version tag at the tested commit and a draft release only after all builds pass. Tags created by the workflow token do not recursively start another workflow. A version tag creates a **draft** release; it will not overwrite a published release's assets.

After reviewing and committing the desired release snapshot:

```sh
# Cargo.toml's version must match the tag.
git tag v0.1.0
git push origin v0.1.0
# Inspect the Actions run and draft release artifacts before publishing.
gh release view v0.1.0 --repo BuenGenio/AgentBanter
```

Only after all jobs pass and the artifacts have been reviewed:

```sh
gh release edit v0.1.0 --draft=false --repo BuenGenio/AgentBanter
```

There is no Apple developer signing/notarization setup in this repository. Builds use Rust's normal macOS linker signing; this is not Developer ID notarization. Native Linux and Intel builds need their CI runs before being described as tested.

## Homebrew tap

The tap repository must exist and contain the formula generated by a completed release build. No placeholder checksums are used. You can prepare an initial public tap with:

```sh
gh repo create BuenGenio/homebrew-tap --public \
  --description 'Homebrew formulae for AgentBanter' --clone
cd homebrew-tap
mkdir -p Formula
gh release download v0.1.0 --repo BuenGenio/AgentBanter \
  --pattern agentbanter.rb --dir Formula
git add Formula/agentbanter.rb
git commit -m 'Add AgentBanter 0.1.0'
git push -u origin HEAD
```

After the release and tap are public, end users can run:

```sh
brew install buengenio/tap/agentbanter
agentbanter open
```

For updates, replace the formula with the next release's generated file. The Homebrew formula supplies its service definition; do not enable both Homebrew services and the separate native systemd unit for the same server. No cross-repository token is needed for release generation; tap updates use the maintainer's normal GitHub authentication.
