# Moaz Elgabry Plugins

`Moaz Elgabry Plugins` is a Tauri 2 desktop manager for `Chromaspace` and `ME_OpenDRT`.

It is designed to:

- ship macOS and Windows manager apps from one codebase
- self-update through Tauri updater artifacts
- install OFX bundles system-wide
- keep plugin release metadata on GitHub Pages
- stay compatible with the current unsigned macOS plugin stage until Developer ID signing is added later

## Current Structure

- `src/`: Vite frontend
- `src-tauri/`: Rust backend, plugin install logic, Tauri config
- `docs/updates/latest.json`: manager update feed
- `docs/plugins/`: plugin catalog and per-plugin manifests
- `docs/release-notes/`: static release notes pages

## Local Development

Prerequisites:

- Node.js LTS
- Rust stable
- Tauri desktop prerequisites for your OS

Commands:

```bash
npm install
npm run tauri:dev
```

Build:

```bash
npm run tauri:build
```

### Local Dev Catalog

In debug builds, the app now prefers:

- `docs/plugins/dev/index.json`

That local catalog points directly at the existing local bundle directories for:

- `Chromaspace`
- `ME_OpenDRT`

This makes it possible to test install/update flow before publishing real release archives.

The local dev manifests use token expansion for:

- `${MEPM_MANAGER_ROOT}`
- `${ME_OFX_ROOT}`
- `${OFX_WORKSHOP_ROOT}`

The production catalog is now expected at:

- `https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/index.json`

Outside the debug local-dev path, the app uses that hosted Pages catalog.

## Updater Configuration

Manager self-update is now wired for signed release artifacts through GitHub Releases and a static updater feed hosted on GitHub Pages.

Before the first real self-update release, add these repository secrets:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

The updater feed is published at:

- `https://moazelgabry.github.io/Moaz-Elgabry-plugins/updates/latest.json`

The Pages workflow regenerates that file from the latest published manager release.

## Plugin Manifests

The committed plugin manifests are placeholders and must be updated before first release with:

- real release asset URLs
- real SHA-256 checksums
- real release notes URLs

## macOS Plugin Install Note

Plugin installation is still designed around the current unsigned stage:

- install into `/Library/OFX/Plugins`
- clear quarantine
- fix ownership/permissions
- apply ad-hoc codesign

That behavior should be removed or reduced once the plugins are signed and notarized properly.
