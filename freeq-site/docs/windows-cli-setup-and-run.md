# Freeq Windows App — Command-Line Setup, Build, and Launch Guide

This guide is for the next developer to bootstrap the native Windows app workflow from the CLI:

1. Set up tooling on Windows.
2. Build the Rust bridge/core (`freeq-windows-core`).
3. Build and run the WinUI app (`freeq-windows-app`).
4. Troubleshoot common issues.

> This repo currently contains architecture/design docs for the Windows app. If `freeq-windows-core/` and `freeq-windows-app/` are not yet present, follow the **Scaffold** section first.

---

## 1) Prerequisites (Windows)

Open **PowerShell 7** as your normal user (not admin unless required by your org policy).

## 1.1 Required software

Install these tools first:

- **Git**
- **Rust toolchain** (stable)
- **Visual Studio 2022 Build Tools** or full VS 2022 with:
  - MSVC C++ toolchain
  - Windows 10/11 SDK
- **.NET SDK 8+**
- **Windows App SDK / WinUI 3 workload** (via Visual Studio installer)
- **Cargo tools**:
  - `cargo-nextest` (optional, faster tests)
  - `cargo-watch` (optional)

Recommended install commands (if `winget` is available):

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.DotNet.SDK.8 -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e
```

After installing Rust:

```powershell
rustup default stable
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup target add x86_64-pc-windows-msvc
```

Verify environment:

```powershell
git --version
rustc -V
cargo -V
dotnet --info
```

---

## 2) Clone and prepare repository

```powershell
git clone https://github.com/<your-org>/freeq.git
cd freeq
```

Optional: use a dedicated branch for Windows work.

```powershell
git checkout -b feat/windows-bootstrap
```

---

## 3) Current repo baseline checks

Before touching Windows-specific projects, verify the workspace is healthy:

```powershell
cargo check
cargo test
```

If tests are too slow locally:

```powershell
cargo test -p freeq-sdk
cargo test -p freeq-tui
```

---

## 4) Scaffold missing Windows projects (if needed)

If these folders already exist, skip to section 5.

- `freeq-windows-core/`
- `freeq-windows-app/`

## 4.1 Create Rust bridge crate

From repo root:

```powershell
cargo new freeq-windows-core --lib
```

Add it to workspace `Cargo.toml` members list.

In `freeq-windows-core/Cargo.toml`, set:

```toml
[lib]
crate-type = ["cdylib", "rlib"]
```

Add dependencies (minimum):

```toml
[dependencies]
freeq-sdk = { path = "../freeq-sdk" }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
anyhow = { workspace = true }
thiserror = { workspace = true }
rusqlite = { workspace = true }
parking_lot = "0.12"
dashmap = "6"
```

## 4.2 Create WinUI app solution

Use Visual Studio templates from CLI (`dotnet new` availability depends on installed templates):

```powershell
mkdir freeq-windows-app
cd freeq-windows-app

# List templates to confirm WinUI template name on your machine
dotnet new list | Select-String -Pattern "winui|windows"
```

If WinUI template exists (example):

```powershell
dotnet new winui3 -n Freeq.Windows.App
```

If templates are missing, create via Visual Studio once, then return to CLI for all builds/runs.

Return to repo root:

```powershell
cd ..
```

---

## 5) Build Rust Windows core from CLI

From repo root:

```powershell
cargo build -p freeq-windows-core --release --target x86_64-pc-windows-msvc
```

Expected artifact:

- `target/x86_64-pc-windows-msvc/release/freeq_windows_core.dll`

(plus `.lib` import library and `.pdb` in debug builds)

For debug iteration:

```powershell
cargo build -p freeq-windows-core --target x86_64-pc-windows-msvc
```

---

## 6) Wire Rust DLL into WinUI app output

Your WinUI app must find `freeq_windows_core.dll` at runtime.

## 6.1 Quick manual copy (works immediately)

Example debug output path:

```powershell
$rustDll = "target/x86_64-pc-windows-msvc/debug/freeq_windows_core.dll"
$appOut  = "freeq-windows-app/Freeq.Windows.App/bin/x64/Debug/net8.0-windows10.0.19041.0"
Copy-Item $rustDll $appOut -Force
```

## 6.2 Preferred: automate in app `.csproj`

Add a post-build target to copy from repo `target` into app output directory.

High-level idea:

- `AfterTargets="Build"`
- `Copy SourceFiles="...freeq_windows_core.dll" DestinationFolder="$(OutDir)"`

Use separate paths for Debug/Release.

---

## 7) Build WinUI app from CLI

From repo root (or app folder):

```powershell
dotnet build .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Debug -p:Platform=x64
```

Release build:

```powershell
dotnet build .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Release -p:Platform=x64
```

---

## 8) Launch app from CLI

Run directly with `dotnet run`:

```powershell
dotnet run --project .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Debug -p:Platform=x64
```

Or run built executable manually:

```powershell
.\freeq-windows-app\Freeq.Windows.App\bin\x64\Debug\net8.0-windows10.0.19041.0\Freeq.Windows.App.exe
```

---

## 9) Suggested developer loop

For day-to-day iteration:

1. Terminal A (Rust core checks):

```powershell
cargo check -p freeq-windows-core
```

2. Terminal B (Rust core build):

```powershell
cargo build -p freeq-windows-core --target x86_64-pc-windows-msvc
```

3. Terminal C (WinUI run):

```powershell
dotnet run --project .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Debug -p:Platform=x64
```

When interop signatures change, rebuild both Rust and .NET sides.

---

## 10) Optional packaging (MSIX) from CLI

Once app packaging project is configured:

```powershell
dotnet publish .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Release -p:Platform=x64
```

If using dedicated packaging project (`.wapproj`), build it with MSBuild:

```powershell
msbuild .\freeq-windows-app\Freeq.Windows.Package\Freeq.Windows.Package.wapproj /p:Configuration=Release /p:Platform=x64
```

---

## 11) Troubleshooting

## 11.1 `DllNotFoundException: freeq_windows_core.dll`

- Confirm DLL copied into app output folder.
- Confirm architecture matches (`x64` app + `x86_64` Rust build).
- Confirm C runtime/toolchain installed.

Check quickly:

```powershell
Get-ChildItem .\freeq-windows-app\Freeq.Windows.App\bin\x64\Debug\net8.0-windows10.0.19041.0\freeq_windows_core.dll
```

## 11.2 `The specified framework 'Microsoft.WindowsAppSDK' was not found`

- Install/repair Windows App SDK + WinUI workload via Visual Studio Installer.
- Reopen terminal after installation.

## 11.3 Rust link errors (`link.exe` not found)

Open “x64 Native Tools Command Prompt for VS 2022” or ensure VS Build Tools are correctly installed.

## 11.4 Interop crashes on callback

- Validate calling convention (`Cdecl` vs `StdCall`) matches Rust export.
- Ensure callback delegate is pinned/not GC-collected.
- Ensure UTF-8 marshaling and null checks.

---

## 12) Minimum CI commands (Windows runner)

Use these in GitHub Actions/Azure Pipelines on `windows-latest`:

```powershell
cargo check -p freeq-windows-core --target x86_64-pc-windows-msvc
cargo test -p freeq-windows-core --target x86_64-pc-windows-msvc

dotnet build .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Release -p:Platform=x64
```

Add artifact upload for:

- Rust DLL output
- WinUI app binaries / MSIX

---

## 13) Quick command reference

From repo root:

```powershell
# Rust
cargo build -p freeq-windows-core --target x86_64-pc-windows-msvc
cargo build -p freeq-windows-core --release --target x86_64-pc-windows-msvc

# .NET / WinUI
dotnet build .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Debug -p:Platform=x64
dotnet run --project .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Debug -p:Platform=x64

# Full fast sanity
cargo check -p freeq-windows-core
dotnet build .\freeq-windows-app\Freeq.Windows.App\Freeq.Windows.App.csproj -c Debug -p:Platform=x64
```

---

## 14) Handoff checklist for next developer

- [ ] All prerequisites installed and verified.
- [ ] `freeq-windows-core` builds in Debug and Release.
- [ ] WinUI app builds in Debug and Release.
- [ ] Rust DLL is copied automatically to app output.
- [ ] `dotnet run` launches app from CLI.
- [ ] Basic connect/send flow works in local test environment.
- [ ] Troubleshooting notes updated with any machine-specific gotchas.

