# Release pipeline and CI for kura (macOS and Linux/WSL), late 2026

Context I checked in the repo: `Cargo.toml` is version 0.2.0, edition 2021. Its dependencies are crossterm 0.29, ratatui 0.30, ratatui-image 11.1 (crossterm + image-defaults), image 0.25, unicode-width, and libc on unix. Nothing uses OpenSSL or `*-sys` C libraries. `.github/workflows/release.yml` triggers on `v*` tags. It uses `actions/checkout@v3`, `actions-rs/toolchain@v1`, `actions/create-release@v1` and `actions/upload-release-asset@v1`, and builds x86_64 linux-gnu, x86_64-apple-darwin **on `macos-latest`** (which is arm64 now, so that target is cross-compiled) and windows-msvc. Assets are zips.

## Which targets to ship

### Takeaway
Ship four targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`. Static musl binaries are the safest choice for "any distro + WSL2". Intel macOS still has a small user base, and you can build it on an arm64 runner without an Intel runner. A universal macOS binary through `lipo` is optional.

### Cited Findings
- GitHub retired the `macos-13` (Intel) runner image. The schedule began 2025-10-01 and full retirement was 2025-12-04. GitHub pointed users to `macos-15-intel`, `macos-14-large` or `macos-15-large`. — [GitHub changelog 2025-09-19](https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/); [GitHub changelog 2025-07-11](https://github.blog/changelog/2025-07-11-upcoming-changes-to-macos-hosted-runners-macos-latest-migration-and-xcode-support-policy-updates/)
- GitHub said Intel macOS support on Actions ends when the macOS 15 image is retired in Fall 2027. A WordPress changeset says `macos-15-intel` is the last x86_64 runner and is supported until August 2027. — [GitHub changelog 2025-09-19](https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/); [WordPress changeset](https://core.trac.wordpress.org/changeset/61241). However, the current runner reference also lists a `macos-26-intel` label (see the runner section), which contradicts the "last Intel image" statement. **Uncertain.**
- Real-world references:
  - yazi builds both `x86_64-apple-darwin` and `aarch64-apple-darwin` on `macos-latest` (arm64 host, cross-compiling to Intel). It ships `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` alongside gnu builds. — [yazi draft.yml](https://raw.githubusercontent.com/sxyazi/yazi/main/.github/workflows/draft.yml)
  - ripgrep ships `x86_64-unknown-linux-musl` (its primary Linux build) and `aarch64-unknown-linux-musl`, plus both darwin targets from `macos-latest`. — [ripgrep release.yml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/.github/workflows/release.yml)
- dist (cargo-dist) supports `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` as "static musl builds". — [dist config reference](https://axodotdev.github.io/cargo-dist/book/reference/config.html)
- musl's default malloc is slow, especially in multi-threaded, allocation-heavy work. The usual fix is mimalloc as `#[global_allocator]` behind `cfg(target_env = "musl")`. Reported gains range from about 10.8x in a synthetic benchmark to about 11% on JPEG-XL decoding. — [Tweag blog](https://www.tweag.io/blog/2023-08-10-rust-static-link-with-mimalloc); [Rust forum: musl slower than gnu](https://users.rust-lang.org/t/why-is-linux-musl-much-slower-than-linux-gnu/136263); [ximei blog 2025](https://blog-ximei-me.pages.dev/blog/en/2025/1203-rust-musl-malloc); [OpenBenchmarking musl malloc](https://mail.openbenchmarking.org/result/2410154-NE-MUSLMALLO78&grr&export=html)
- On Apple Silicon, binaries must be signed, but an ad-hoc signature is enough, and the macOS linker adds one automatically. — [Rust users forum](https://users.rust-lang.org/t/distributing-cli-apps-on-macos/70223)
- taiki-e/upload-rust-binary-action accepts `universal-apple-darwin` as a target and produces a lipo'd universal binary. — [upload-rust-binary-action README](https://github.com/taiki-e/upload-rust-binary-action)

### Inferences
- kura's dependency tree is pure Rust: the image codecs, png/jpeg/gif decoders, crossterm, and the libc crate bindings, with no C libraries. So musl should build cleanly with `rustup target add x86_64-unknown-linux-musl` on an x86_64 Ubuntu runner, and with `aarch64-unknown-linux-musl` on `ubuntu-24.04-arm`. Installing `musl-tools` via apt is a cheap safeguard. Rust's musl targets ship self-contained CRT objects, so pure-Rust crates often link without musl-tools. I have not verified this for kura; test it.
- The musl malloc penalty probably matters little for kura. It decodes one preview image at a time and mostly waits on user input. Adding `mimalloc` under `cfg(target_env="musl")` is optional. Only do it if image previews feel slow on Linux.
- The `x86_64-apple-darwin` cross-compile on an arm64 runner works out of the box with `rustup target add`, as yazi and ripgrep do. kura's Carbon/CoreFoundation FFI links Apple system frameworks, which exist for both architectures in the macOS SDK. No Intel runner is needed. One drawback: an arm64 runner cannot run the x86_64 tests natively. Rosetta might allow it, but I have not verified Rosetta on GitHub runners.
- Separate per-arch tarballs are simpler and follow the yazi/ripgrep convention. A universal binary is only worth it if you want a single macOS asset (for example, for a simpler Homebrew formula).
- Dropping gnu Linux builds is reasonable. A static musl binary has no glibc version floor, which avoids "GLIBC_2.xx not found" on older WSL distros. This is an inference from how static linking works; no source above tested it.

### Gaps
- I did not verify whether `macos-15-intel` and `macos-26-intel` coexist in practice, or the exact Intel sunset date beyond the "Fall 2027" / "August 2027" statements.
- I found no data on how many kura-like users are still on Intel Macs.
- I did not test kura's musl build. Watch `ratatui-image`'s feature graph: chafa is disabled, which is good, because chafa would pull in a C library.

## Tooling options and their status

### Takeaway
For a hobby project there are two good paths:
- **(A)** A hand-written workflow using `dtolnay/rust-toolchain` + `Swatinem/rust-cache@v2` + native runners + `softprops/action-gh-release` (or the `gh` CLI). This has the fewest moving parts and is easy to read.
- **(B)** `dist` (axodotdev, actively released). It generates the whole release.yml plus a curl|sh installer and a Homebrew formula/tap.

taiki-e's action pair sits in between: one action per job, with built-in checksums and a universal-darwin option. Since every target can be built natively (arm64 and x86_64 Linux runners, arm64 macOS), kura doesn't need cross-rs or zigbuild.

### Cited Findings
- **dist (axodotdev, formerly cargo-dist):** releases 0.31.0 (2026-02-23), 0.32.0 (2026-05-21), 0.33.0 (2026-09-10), 0.33.1 (2026-10-21), 0.34.0 (2026-11-01, latest). The repo shows about 303 open issues and 36 open PRs. — [dist releases](https://github.com/axodotdev/cargo-dist/releases)
  - Conflict: the 0.34.0 notes say it "upgrades the aarch64-apple-darwin runner to macos-14", but the linked PR is titled "default Apple Silicon builds to macos-15". — [dist releases](https://github.com/axodotdev/cargo-dist/releases)
  - Workflow: run `dist init`, which writes the dist config (Cargo.toml metadata or dist-workspace.toml), a shippable build profile, and `.github/workflows/release.yml`. Pushing a version tag such as `v0.1.0` triggers CI, which creates the GitHub Release. `dist plan` runs on PRs by default. — [dist quickstart](https://axodotdev.github.io/cargo-dist/book/quickstart/rust.html)
  - Config: `targets` takes a list of triples. `installers` takes `"shell"`, `"homebrew"`, `"powershell"`, `"npm"` or `"msi"`. `tap` names the Homebrew repo; it must exist and the token needs write access to it. `checksum` defaults to `"sha256"`. `install-path` defaults to `CARGO_HOME` (`~/.cargo/bin`). `github-custom-runners` overrides the runner per target. `pr-run-mode` is `plan`, `skip` or `upload`. — [dist config reference](https://axodotdev.github.io/cargo-dist/book/reference/config.html)
- **taiki-e/upload-rust-binary-action@v1** (paired with taiki-e/create-gh-release-action@v1):
  - Inputs: `bin`, `target`, `checksum` (sha256/sha512/b2/…), `archive` (default `$bin-$target`, with `$tag` available), `tar` (default unix), `zip` (default windows), `build-tool` (`cargo`/`cross`/`cargo-zigbuild`, auto-chosen by default, with cross used for targets it supports), and the `universal-apple-darwin` target.
  - Jobs that create or upload to a release need `contents: write`.
  - Source: [upload-rust-binary-action](https://github.com/taiki-e/upload-rust-binary-action)
- **houseabsolute/actions-rust-cross@v1:** uses `cross` on Linux except for x86/x86_64 targets, and plain cargo on macOS and Windows. Inputs: `target`, `args`, `strip`, `cross-version`, `force-use-cross`, `use-rust-cache`. Its companion `houseabsolute/actions-rust-release@v0` packages and releases. There was README activity in March 2026. — [actions-rust-cross](https://github.com/houseabsolute/actions-rust-cross)
- **Swatinem/rust-cache@v2** is the documented tag. Install the toolchain *before* the cache step, because the cache key includes the rustc version. — [rust-cache README](https://github.com/Swatinem/rust-cache)
- **actions/checkout:** the latest major is v7 (v7.0.0 on June 18, v7.0.1 on July 20, year presumably 2026). — [checkout releases](https://github.com/actions/checkout/releases)
- **yazi (current) uses:**
  - `actions/checkout@v7`, `dtolnay/rust-toolchain@stable`, `actions/upload-artifact@v7`, `actions/download-artifact@v8`, `softprops/action-gh-release@v3`, sccache.
  - musl builds run in `ghcr.io/cross-rs/<target>:edge` containers.
  - Source: [yazi draft.yml](https://raw.githubusercontent.com/sxyazi/yazi/main/.github/workflows/draft.yml)
- **ripgrep uses:**
  - `cross` 0.2.5 (`CARGO=cross`) for Linux cross targets, including musl.
  - The `gh` CLI for releases: `gh release create $VERSION --draft --verify-tag`, then `gh release upload`.
  - Permissions `contents: write`, `id-token: write` and `attestations: write`, for build provenance.
  - Source: [ripgrep release.yml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/.github/workflows/release.yml)

### Inferences
- `actions-rs/*` is unmaintained, and `actions/create-release` and `actions/upload-release-asset` are archived. This is widely known but I did not re-fetch those repos this session. Replace them with `dtolnay/rust-toolchain@stable` plus `softprops/action-gh-release@v3` or `gh release`.
- Recommended for kura: native runners (`macos-latest` for both darwin targets, `ubuntu-24.04` for x86_64 musl, `ubuntu-24.04-arm` for aarch64 musl). With these runners kura needs neither `cross` nor zigbuild, avoiding Docker and the long-unreleased `cross` 0.2.5.
- Choose dist only if you want the curl|sh installer and a Homebrew tap today. The cost is a generated ~300-line workflow that you regenerate with `dist init` rather than hand-edit, plus a version-coupled tool. For a hobby project, hand-written (A) is easier to understand. You can switch to dist later once a Homebrew tap matters.
- The taiki-e pair is a good middle ground. It does tar.gz + sha256 + upload in a single step per matrix entry, and it can produce `universal-apple-darwin`.

### Gaps
- I did not fetch the cross-rs or cargo-zigbuild repos, so their 2026 release status is unverified. They aren't needed for the recommended plan.
- I did not confirm the latest softprops/action-gh-release major beyond yazi's use of `@v3`, or the actions/upload-artifact and download-artifact majors beyond yazi's `@v7` and `@v8`.
- I did not confirm the taiki-e and houseabsolute release dates.

## GitHub-hosted runner labels and pricing (2026)

### Takeaway
Standard runners are free and unlimited for public repos. Use `macos-latest` (arm64, currently macOS 26) or pin `macos-15`. Use `ubuntu-24.04` for x64 and `ubuntu-24.04-arm` for arm64 Linux. Intel macOS runs on `macos-15-intel` (or `macos-26-intel`), but kura doesn't need it.

### Cited Findings
These are the labels on GitHub's runner reference page, which also says standard runners are "free and unlimited on public repositories." — [GitHub Docs: GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)

- Linux x64:
  - `ubuntu-latest` (= 24.04), `ubuntu-24.04`, `ubuntu-22.04`, `ubuntu-26.04`: 4 CPU / 16 GB.
  - `ubuntu-slim`: 1 CPU, containerized.
- Linux arm64: `ubuntu-24.04-arm`, `ubuntu-22.04-arm`, `ubuntu-26.04-arm`.
- macOS arm64:
  - `macos-latest` (currently macOS 26), `macos-15`, `macos-26`.
  - Spec: 3 CPU (M1) / 7 GB.
- macOS Intel: `macos-15-intel`, `macos-26-intel` (4 CPU / 14 GB).

### Inferences
- Pinning `macos-15` / `ubuntu-24.04` makes builds reproducible. `-latest` labels shift during image migrations.
- The kura repo appears to be public (hanafsky/Kura on GitHub), so CI costs nothing. If it is private, macOS minutes carry a multiplier. I did not fetch the pricing page.

### Gaps
- I didn't fetch private-repo pricing.
- The `macos-26-intel` label conflicts with the claim that `macos-15-intel` is the last Intel image (see above).

## macOS distribution pitfalls

### Takeaway
An unsigned binary from GitHub Releases works:
- **curl / Homebrew / shell-installer:** no quarantine flag is set, so it runs directly.
- **Browser download:** Gatekeeper blocks it. Users must run `xattr -d com.apple.quarantine ./kura` or allow it in System Settings.

Notarization needs a paid Apple Developer ID ($99/yr) and isn't required. Document the xattr workaround in the README and offer a curl or Homebrew path.

### Cited Findings
- Unix networking tools such as curl and scp don't quarantine files they download, so Gatekeeper's check doesn't fire. — [Apple Developer Forums: Resolving Trusted Execution Problems](https://developer.apple.com/forums/thread/706442)
- You can remove the flag with `xattr -d com.apple.quarantine <path>`. — [Eclectic Light: quarantine flag](https://eclecticlight.co/2020/10/29/quarantine-and-the-quarantine-flag/); [Apple Developer Forums](https://developer.apple.com/forums/thread/706442)
- An ad-hoc signature, applied automatically by the linker, is enough for an arm64 binary to execute. — [Rust users forum](https://users.rust-lang.org/t/distributing-cli-apps-on-macos/70223)
- A 2026 report says Homebrew installs can still trigger Gatekeeper prompts in some scenarios (a cask with a quarantined Caskroom). This is anecdotal. — [Cursor forum](https://forum.cursor.com/t/cursor-cli-installation-method-zsh-vs-bash/158601)
- dist can publish a Homebrew formula to a `homebrew-*` tap repo, which must already exist and requires a token with write access. — [dist config reference](https://axodotdev.github.io/cargo-dist/book/reference/config.html)

### Inferences
- Use a Homebrew **formula** (not a cask) in a personal tap such as `hanafsky/homebrew-tap`. Formula installs are the common path for CLI tools and usually avoid the quarantine prompt.
- Don't strip binaries in a way that breaks the ad-hoc signature. `strip` on macOS can invalidate it. Setting `[profile.release] strip = true` in Cargo is handled by rustc, which re-signs. **I did not verify this re-signing in this session; test that the downloaded arm64 binary runs.**

### Gaps
- Not researched: notarization of a bare CLI (it requires a zip/pkg submission via `notarytool`), and how Gatekeeper on macOS 26 now treats unsigned CLIs.

## Checksums, naming, tag trigger, permissions, version check

### Takeaway
Trigger on `push: tags: ['v*']`. Set `permissions: contents: write` on the release job. Verify that the tag equals the `Cargo.toml` version before building. Name archives `kura-<tag>-<target>.tar.gz` (or `kura-<target>.tar.gz`) and publish a `.sha256` per archive, or a combined `SHA256SUMS`. ripgrep, yazi, taiki-e and dist all follow this pattern.

### Cited Findings
- ripgrep checks the version with `if ! grep -q "version = \"$VERSION\"" Cargo.toml; then echo "version does not match Cargo.toml" >&2; exit 1`. It creates checksums with `shasum -a 256 "$ARCHIVE.tar.gz" > "$ARCHIVE.tar.gz.sha256"`. Releases are drafts via `gh release create --draft --verify-tag`. — [ripgrep release.yml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/.github/workflows/release.yml)
- ripgrep also requests `id-token: write` and `attestations: write` for build provenance attestation (`actions/attest-build-provenance`). — [ripgrep release.yml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/.github/workflows/release.yml)
- taiki-e's archive name defaults to `$bin-$target`. `$tag` is available, and `checksum: sha256` is supported. The workflow sets `contents: read` at top level and `contents: write` on release jobs. — [upload-rust-binary-action](https://github.com/taiki-e/upload-rust-binary-action)
- yazi creates a **draft** release with generated notes on tag push and attaches `yazi-${target}.zip`. — [yazi draft.yml](https://raw.githubusercontent.com/sxyazi/yazi/main/.github/workflows/draft.yml)
- dist's checksum default is sha256. — [dist config reference](https://axodotdev.github.io/cargo-dist/book/reference/config.html)

### Inferences
- Prefer `.tar.gz` over `.zip` on unix. tar preserves the executable bit, and kura's current zips may lose it depending on the unzip tool.
- Keep the version check robust. A good form is `cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'` compared against `${GITHUB_REF_NAME#v}`. It's more precise than grep, which could match a dependency's version line.
- Creating draft releases lets you inspect them before publishing, which is a cheap safety net.

### Gaps
- None significant.

## Lightweight CI on push/PR

### Takeaway
One workflow on `push` (main) and `pull_request`:
- A matrix of `ubuntu-24.04` + `macos-latest`.
- Steps: `actions/checkout@v7`, `dtolnay/rust-toolchain@stable` (with `components: rustfmt, clippy`), `Swatinem/rust-cache@v2`, then `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Permissions: `contents: read`.

### Cited Findings
- The rust-cache README requires installing the toolchain before the cache step, because the cache key uses the rustc version. — [rust-cache](https://github.com/Swatinem/rust-cache)
- yazi uses `dtolnay/rust-toolchain@stable`; checkout's current major is v7. — [yazi draft.yml](https://raw.githubusercontent.com/sxyazi/yazi/main/.github/workflows/draft.yml); [checkout releases](https://github.com/actions/checkout/releases)

### Inferences
- Run fmt once (on Linux only) and clippy/test on both OSes. That exercises the macOS-only `input_source.rs` FFI and the unix `libc` paths.
- Optionally add `cargo build --target x86_64-unknown-linux-musl` to CI so musl breakage shows up before a release.
- A `-D warnings` clippy gate can break when a new stable Rust adds lints. That's acceptable for a hobby project; pin the toolchain if it becomes annoying.

### Gaps
- I didn't check whether kura's tests need a TTY. If they do, CI tests may need gating.

## WSL specifics

### Takeaway
A static `x86_64-unknown-linux-musl` binary should run in any WSL2 distro, because WSL2 runs a real Linux kernel and the binary has no libc dependency.

Image previews: Windows Terminal ≥ 1.22 (stable since Feb 2025) supports Sixel, so ratatui-image's Sixel path should work from WSL inside WT.

Opening files: `xdg-open` usually isn't set up in WSL, and `wslview` (wslu) is discontinued. Detect WSL and run `explorer.exe "$(wslpath -w <file>)"` (or `cmd.exe /c start "" <winpath>`).

### Cited Findings
- Sixel support shipped in Windows Terminal Preview 1.22 (Aug 2024), contributed by j4james. Stable WT 1.22 with Sixel arrived in Feb 2025. — [WT Preview 1.22 blog](https://devblogs.microsoft.com/commandline/windows-terminal-preview-1-22-release); [WT Preview 1.23 blog (stable → 1.22)](https://devblogs.microsoft.com/commandline/windows-terminal-preview-1-23-release/)
- wslu, which provides `wslview`, is unmaintained. A Canonical engineer wrote in Dec 2025 that upstream "discontinued the project and org in GitHub", and a bug proposes dropping it from the Ubuntu archive. DDEV removed wslu in Apr 2026. — [Ubuntu archive bug 2131669](https://lists.snapcraft.io/archives/ubuntu-archive/2025-December/113453.html); [DDEV PR #8351](https://github.com/ddev/ddev/pull/8351); [wslu GitHub](https://Github.com/wslutilities/wslu)
- Microsoft documents `wslpath` (with `-w` giving a Windows/`\\wsl.localhost\...` path) and `explorer.exe .`. From WSL you must call Windows executables with the `.exe` suffix. Interop can be disabled in `/etc/wsl.conf` `[interop]`. — [MS Learn: WSL interop](https://learn.microsoft.com/en-us/windows/dev-environment/wsl-interop); [MS Learn: WSL filesystems](https://learn.microsoft.com/en-us/windows/wsl/filesystems)
- The community-documented pattern is `explorer.exe "$(wslpath -w /path/to/file)"`, with `cmd.exe /c start "" "<winpath>"` as an alternative. Community notes report that `explorer.exe <dir>` doesn't open directories reliably without `wslpath -w`. — [WSL interop skill (community)](https://cdn.jsdelivr.net/gh/JeremiahChurch/claude-skill-wsl-interop@main/SKILL.md); [zenn scrap (JP)](https://zenn.dev/nodamushi/scraps/c2446e57a61d10)

### Inferences
- WSL detection in kura (src code, not CI): check the `WSL_DISTRO_NAME` env var, or whether `/proc/sys/kernel/osrelease` contains "microsoft". If running under WSL, use `explorer.exe` + `wslpath -w` instead of `xdg-open`. This is a code change, separate from the release pipeline.
- ffmpeg/ffprobe must be installed inside the WSL distro (`apt install ffmpeg`). Windows-side ffmpeg.exe would also work through interop but with Windows paths. Document this.
- ratatui-image detects the graphics protocol by querying the terminal. In WT it should pick Sixel. Older conhost or WT < 1.22 would fall back to halfblocks.

### Gaps
- I haven't verified that ratatui-image 11's protocol/font-size probing (CSI queries) works correctly in WT through WSL. Test it manually.
- Not researched: whether WT supports the Kitty graphics protocol as of late 2026.
- I did not confirm aarch64 WSL (Windows on ARM) usage, which would need the `aarch64-unknown-linux-musl` binary.
