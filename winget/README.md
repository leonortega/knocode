# Winget manifests for Knocode

This folder holds the [Windows Package Manager](https://learn.microsoft.com/windows/package-manager/) (winget) manifests for Knocode, laid out exactly as required by the [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) repository:

```
Knocode/knocode/<version>/
  Knocode.installer.yaml      # InstallerType: portable (zip with knocode.exe + knocode-daemon.exe)
  Knocode.locale.en-US.yaml
  Knocode.yaml
```

The release workflow regenerates this folder for each new tag (`v0.9.6`, `v0.9.7`, ...) with the
correct version, URL, and SHA-256, and commits it to `dev`. Older version folders are removed.

## Submitting to winget-pkgs (one-time + per new version)

winget packages live in the external `microsoft/winget-pkgs` repository, so publication is a
pull request there — it cannot be automated from this repo alone. Per new version:

```powershell
# 1. Install the helper
winget install wingetcreate   # or: dotnet tool install --global wingetcreate

# 2. Point it at the release zip (it validates URL + hash against the GitHub release)
wingetcreate update Knocode.knocode --version <VERSION> --urls "https://github.com/leonortega/knocode/releases/download/v<VERSION>/knocode-<VERSION>-x86_64-pc-windows-msvc.zip"

# 3. Follow its prompts to open the PR in microsoft/winget-pkgs
```

Or copy this folder into a clone of `microsoft/winget-pkgs` at
`manifests/k/Knocode/knocode/<version>/` and open the PR manually (see
[CONTRIBUTING.md](https://github.com/microsoft/winget-pkgs/blob/master/CONTRIBUTING.md)).

Once merged, users install with:

```powershell
winget install Knocode.knocode
winget upgrade Knocode.knocode
```

> Note: the winget validation bot requires `LicenseUrl` to resolve to actual license text —
> the repo ships an MIT `LICENSE` file at the root for that reason.
