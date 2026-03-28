# Plugin Manager Development Notes

This file is the quick background note for future work on the Moaz Elgabry plugin manager. The goal is that a single glance here should explain how the project is organized, what the current constraints are, how releases and manifests are generated, and what to watch out for before changing behavior.

Update this file whenever any of the following change:
- repo relationships
- release/workflow behavior
- platform-signing status
- updater/catalog URLs
- manifest schema or channel rules
- important UX behavior that future work should preserve

## Purpose

The plugin manager is a Tauri desktop app that:
- presents a catalog of OFX plugins
- installs, updates, reinstalls, downgrades, and uninstalls plugin bundles
- hosts its public catalog and updater feed from GitHub Pages
- consumes plugin releases from separate plugin repos

Current managed plugins:
- Chromaspace
- ME_OpenDRT

## Main Repositories

Manager repo:
- GitHub: https://github.com/MoazElgabry/Moaz-Elgabry-plugins
- Local path: `C:\Users\mizo_\OneDrive\Documents\GitHub\Moaz-Elgabry-plugins`

Chromaspace repo:
- GitHub: https://github.com/MoazElgabry/Chromaspace
- Local path: `C:\Users\mizo_\OneDrive\Documents\GitHub\Chromaspace`

ME_OpenDRT public release repo:
- GitHub: https://github.com/MoazElgabry/ME_OpenDRT-OFX
- Local path currently used for local work: `C:\Users\mizo_\OneDrive\Documents\GitHub\ME_OFX`
- Important note: the local folder name is `ME_OFX`, but the public GitHub repo is `ME_OpenDRT-OFX`

Other related local path used by manifests/builds:
- OFX workshop root: `C:\Users\mizo_\OneDrive\Documents\GitHub\OFX-Workshop`

## App Stack

Frontend:
- Vite
- plain JavaScript
- CSS in `src/styles.css`

Desktop shell:
- Tauri v2

Backend:
- Rust in `src-tauri/src`

Important frontend files:
- `index.html`
- `src/main.js`
- `src/styles.css`

Important backend files:
- `src-tauri/src/catalog.rs`
- `src-tauri/src/installer.rs`
- `src-tauri/src/models.rs`
- `src-tauri/src/settings.rs`

## Public Hosting And Feeds

Catalog index:
- `https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/index.json`

Updater feed:
- `https://moazelgabry.github.io/Moaz-Elgabry-plugins/updates/latest.json`

Important generated docs paths:
- `docs/plugins/index.json`
- `docs/plugins/chromaspace/stable.json`
- `docs/plugins/chromaspace/beta.json`
- `docs/plugins/me-opendrt/stable.json`
- `docs/plugins/me-opendrt/beta.json`
- `docs/updates/latest.json`

Local dev-only feed paths:
- `docs/plugins/dev/index.json`
- `docs/plugins/dev/chromaspace.local.json`
- `docs/plugins/dev/me-opendrt.local.json`

Important note:
- the live manager does not read GitHub releases directly
- it reads generated manifest JSON and the generated updater feed

## Build And Local Preview Commands

Run local dev preview:

```powershell
cd "C:\Users\mizo_\OneDrive\Documents\GitHub\Moaz-Elgabry-plugins"
npm.cmd run tauri:dev
```

Build production-style local package:

```powershell
cd "C:\Users\mizo_\OneDrive\Documents\GitHub\Moaz-Elgabry-plugins"
npm.cmd run tauri:build
```

Frontend build check:

```powershell
cd "C:\Users\mizo_\OneDrive\Documents\GitHub\Moaz-Elgabry-plugins"
npm.cmd run build
```

Rust build check:

```powershell
cd "C:\Users\mizo_\OneDrive\Documents\GitHub\Moaz-Elgabry-plugins\src-tauri"
cargo check
```

Generate plugin manifests manually:

```powershell
cd "C:\Users\mizo_\OneDrive\Documents\GitHub\Moaz-Elgabry-plugins"
npm.cmd run generate:plugin-manifests
```

## Manager Release Workflows

Manager build workflow:
- `.github/workflows/build-plugin-manager.yml`

Manager Pages deployment workflow:
- `.github/workflows/deploy-plugin-manager-pages.yml`

Current manager build workflow behavior:
- triggers on pushes to `main` for manager-app-relevant files
- checks the app version from `package.json`
- uses version tag format `plugin-manager-v<version>`
- creates or reuses a draft release shell for that version
- skips cleanly when that version is already drafted/published
- builds Windows, macOS, and Linux packages
- uploads updater artifacts used by the Tauri updater

Current Pages workflow behavior:
- triggers on:
  - workflow dispatch
  - pushes touching `docs/**`
  - manager release `published`
- regenerates plugin manifests from the latest plugin release data
- regenerates the manager updater feed
- publishes Pages content from `pages-dist`

Important practical note:
- a manager release can exist before `updates/latest.json` has refreshed
- in that gap the manager may report that the update is still being published
- this is expected race behavior until Pages catches up

## Plugin Repo Release Automation

Each plugin repo owns its own release artifacts.

Each plugin repo has:
- a `manager-release-config.json`
- an `update-plugin-manager-manifest.yml` workflow

That workflow:
- reads the plugin repo release data
- regenerates the manager manifest content
- opens a PR against `Moaz-Elgabry-plugins`

Secrets required in each plugin repo:
- `MEPM_MANAGER_REPO_TOKEN`

Token purpose:
- allows the plugin repo workflow to open/update PRs in `Moaz-Elgabry-plugins`

## Channel Model

Current supported channels:
- stable
- available stable history
- beta

Not implemented yet:
- dev distribution

Current rules:
- `stable.json` exposes the current public stable release
- `availableVersions` contains older stable versions explicitly marked to remain installable
- `beta.json` exposes the latest public prerelease

Stable-history marker in a published plugin release body:

```text
manager-available-stable: true
```

Release highlights block in a published plugin release body:

```md
<!-- manager-highlights:start -->
- Bullet one
- Bullet two
Short optional note.
<!-- manager-highlights:end -->
```

That block is extracted into:
- top-level `releaseHighlights` for the current release
- per-version `releaseHighlights` for entries inside `availableVersions`

## Important Current UX/Behavior Decisions

- If beta is enabled and a beta manifest exists, beta becomes the target latest release.
- Stable history should still remain selectable when beta is enabled.
- If a user has a beta installed and later disables beta, the card should help them move back to stable rather than pretending the beta is simply "up to date".
- Version-history UI should stay hidden unless there is more than one selectable version.
- Release highlights are shown through compact info buttons in the main action row and version-history row.
- Manager auto-update is checked opportunistically before plugin install/update actions, but plugin install remains the primary user intent:
  - manager update failures should not block plugin install/update
  - manager update failures can still be shown afterward for diagnosis

## Plugin Package Layout Expectations

The manager expects plugin packages to contain the `.ofx.bundle` at the top level of the archive.

Examples:
- Windows plugin package: portable `.zip`
- macOS plugin package: portable `.zip`
- Linux plugin package: portable `.tar.gz` or other supported archive type as described in the manifest

The manager installs bundles into platform-specific OFX locations and uses admin elevation where needed.

## macOS Signing / Notarization Status

Current limitation:
- you are not currently an Apple Developer Program member
- because of that, you cannot properly code-sign and notarize macOS app bundles and installers for full Gatekeeper-friendly distribution

What this means today:
- macOS artifacts can still be built and distributed
- but they should be treated as unsigned or not fully notarized from Apple's perspective
- macOS users may need manual trust steps such as right-click Open, Security settings approval, or similar Gatekeeper bypass behavior

Current practical workaround:
- keep the release process as smooth as possible on Windows and Linux
- still produce macOS packages so they are available
- avoid pretending the macOS distribution is fully signed/notarized until proper Apple credentials exist

Future direction when you become an Apple Developer:
- add proper Apple signing certificates and notarization to CI
- sign the manager app bundles correctly
- notarize DMG/app artifacts
- review whether plugin bundles themselves should also be signed where beneficial
- remove or reduce user-facing macOS trust friction once notarized distribution is in place

## Tauri Updater Notes

Important updater config lives in:
- `src-tauri/tauri.conf.json`

Current updater endpoint:
- `https://moazelgabry.github.io/Moaz-Elgabry-plugins/updates/latest.json`

Current Windows updater mode:
- passive install mode in Tauri updater config

Current NSIS manager bundle behavior:
- current-user install mode in Tauri bundle config

## Known Repo/Workflow Conventions

Manager version sources must stay aligned in:
- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

When bumping manager version, keep those three in sync.

When plugin releases change and the manager does not see them:
- check whether the plugin repo manifest-update workflow ran
- check whether it opened a PR in `Moaz-Elgabry-plugins`
- check whether the PR was merged
- check whether Pages deployed afterward

When a published plugin release is edited after publishing:
- manifest automation should react to release edits as well as initial publish

## Future Improvement Directions

- Proper macOS signing + notarization once Apple developer credentials are available
- Better release-feed race handling so manager updates feel more immediate after release publish
- More resilient CI around draft release reuse and asset replacement
- Cleaner dev-preview channel design if private or password-gated previews are needed later
- Continued UI polishing for narrow mode, sticky regions, and scroll/fade behavior
- Better semantic handling of beta-to-stable transitions and version-history wording

## Maintenance Reminder

Whenever you change:
- a release workflow
- a manifest schema
- signing behavior
- updater endpoint behavior
- local dev feed behavior

update this note at the same time so future work stays grounded in the real setup instead of stale memory.
