# Domus Obscura - Agent instructions

## Commands

Use the pins in rust-toolchain.toml, .node-version and package.json.
On Windows, use the launcher to select the complete MSVC environment.
It accepts the stable Rust alias only when its exact version matches the pin.

```powershell
.\dev.ps1 bootstrap
.\dev.ps1 dev
.\dev.ps1 test backend_configuration
.\dev.ps1 contracts
.\dev.ps1 check
.\dev.ps1 build
.\dev.ps1 smoke
.\dev.ps1 scale
```

On Linux, use the same tasks:

```sh
cargo task bootstrap
cargo task dev
cargo task test backend_configuration
cargo task contracts
cargo task check
cargo task build
cargo task smoke
cargo task scale
```

For documentation-only changes, use the formatter check:

```sh
pnpm format:check
```

For an isolated browser harness with fresh generated state:

```powershell
.\dev.ps1 local-test-build
.\dev.ps1 local-test
```

Read [detailed agent operations](docs/agent-operations.md) before changing a CLI,
provider, worker, publication, recovery or MCP operation. It preserves the original
command-specific safety rules. See [local development](docs/local-development.md)
for the real development HTTPS setup. Do not treat the harness as a production mode.

## Layout

| Path                            | Purpose                                                |
| ------------------------------- | ------------------------------------------------------ |
| crates/domus/src/domain         | Paths, jobs, editions and output policy                |
| crates/domus/src/application    | Application services and use cases                     |
| crates/domus/src/infrastructure | SQLite, native files, providers and guarded API        |
| crates/domus/migrations         | Ordered SQLite schema changes                          |
| crates/domus/tests              | Isolated workflows, protocol fixtures and scale checks |
| frontend/app                    | Connected React application                            |
| frontend/src/generated          | Rust-generated TypeScript contracts                    |
| frontend/local-test             | Separate sample-data browser harness                   |
| xtask                           | Shared development tasks and native JPEG setup         |
| PRODUCT.md                      | Product scope, confirmed UI and open decisions         |
| docs/ui-poc                     | Approved UI study and Component library                |

## Changing a shared rule - Checklist

1. Read PRODUCT.md and the relevant detailed operational rules.
2. Change the authoritative Rust rule and add an isolated regression test.
3. **Regenerate affected TypeScript contracts; never edit generated files.**
4. Run the targeted test, then the full check before delivery.
5. Run the separate scale task after index or query changes.
6. Update directly related documentation and the linked issue.

## CI Pipeline

- **Check and build** runs contracts, rustfmt, Clippy with denied warnings, Rust tests, harness tests, Prettier, frontend types/tests, builds and smoke on Windows and Linux.
- **Million-record pagination** runs separately on both systems.
- Report local results separately from remote results for the current commit.

## Conventions

| Concern                    | Rule                              |
| -------------------------- | --------------------------------- |
| Rust                       | rustfmt; four-space indentation   |
| Frontend and documentation | Prettier; two-space indentation   |
| Text files                 | UTF-8; LF; final newline          |
| Lockfiles                  | Cargo.lock and one pnpm-lock.yaml |

## Operational boundaries

Windows distribution tasks are `package-windows`, `package-check`, and
`installer-windows`. Read [Windows operations](docs/windows-services.md) before
changing installer or service behavior. NSIS compilation is not installation
or native lifecycle proof. Do not install tools, create accounts, change ACLs,
register services, or reboot a host without separate approval.

- Use generated or permitted fixtures without live credentials for local checks.
- Get separate approval for live downloads, existing-library writes, external configuration, software installation and public exposure.
- Do not deploy or call this foundation the complete V1 application.
- Preserve user-authored guidance and the approved UI study.
- Do not modify the previous music-organizer repository.
