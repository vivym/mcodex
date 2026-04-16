# Mcodex Pooled Startup And Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the pooled-startup source-of-truth fix and the `mcodex` product-identity migration flow together so fresh and migrated `mcodex` homes start correctly without manual `config.toml` edits, while preserving upstream-friendly config/state boundaries and additive public contracts.

**Architecture:** Keep pooled startup facts in `codex-state`, move the shared startup-status/applicability adapter into `codex-account-pool`, and make every surface consume that shared result instead of open-coding config/state probes. In parallel, centralize product identity metadata for home/env names plus system/admin config roots, switch runtime home identity to `MCODEX_HOME`/`~/.mcodex`, add a first-run product migration service that runs before personality migration, and explicitly treat pooled startup selection as installation-local state that is not imported across the product boundary.

**Tech Stack:** Rust workspace crates (`codex-product-identity`, `codex-utils-home-dir`, `codex-core`, `codex-core-skills`, `codex-state`, `codex-account-pool`, `codex-cli`, `codex-app-server`, `codex-app-server-protocol`, `codex-tui`), SQLite via `codex-state`, existing startup/migration hooks, install scripts, `pretty_assertions`, `insta` where needed, and targeted crate tests.

---

## Scope

In scope:

- switch active runtime home resolution from `CODEX_HOME`/`~/.codex` to `MCODEX_HOME`/`~/.mcodex`
- centralize product identity metadata and route runtime system/admin config roots, macOS managed-preferences domain, legacy managed-config shims, and admin skills roots through it so normal `mcodex` startup does not read upstream `/etc/codex/...`, `%ProgramData%\OpenAI\Codex\...`, or `com.openai.codex` roots/domains by default
- add first-run product-identity migration detection, marker persistence, config/auth import, and explicit disclosure that pooled startup selection is not imported
- run product-identity migration before personality migration, with separate markers and non-overlapping skip semantics
- extend pooled startup resolution into an additive startup-status surface shared by state/account-pool/CLI/core/app-server/TUI
- make explicit local config/default-pool intent actionable on fresh homes once the local state runtime is initialized
- remove the false dependency on `[accounts]` presence for pooled startup and app-server lease diagnostics
- preserve existing JSON/protocol field meanings while adding resolution/status fields
- update `accounts add --account-pool ...` to establish persisted default selection only when no durable config/state default exists
- update installer/update edges and docs to the `mcodex` identity

Out of scope:

- DB-only account-pool redesign
- remote account-pool backend implementation
- redesigning `codex login/logout/status`
- changing existing `effectivePoolSource` or `configuredPoolCount` semantics
- breaking `app-server` v2 payload contracts
- workspace-wide `cargo test` without explicit user approval

## Identity Storage Boundary

Keep product-private state separate from reusable user tooling:

- `~/.mcodex` is the active product home and owns auth, SQLite state, logs, account-pool runtime state, embedded system-skill cache, marketplace/plugin caches, themes, and other `mcodex`-private runtime artifacts.
- `~/.agents/skills` remains the cross-product user skill location. Do not move or hide it when switching the active product home.
- repo-local `.agents/skills` and `.agents/plugins` remain project-scoped extension locations and should not be renamed as part of this product identity slice.
- legacy `$CODEX_HOME/skills` / `~/.codex/skills` may be offered as an explicit one-time import source, but normal `mcodex` runtime must not share it as a live fallback.
- legacy marketplace/plugin caches under `~/.codex` are not shared at runtime. Importing them, if needed, should be a separate explicit task because plugin cache format and remote sync policy can evolve independently.

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Run the targeted tests listed in each task before `just fmt` or `just fix -p ...`.
- Run `just fmt` from `codex-rs/` after each Rust code task.
- Run `just fix -p <crate>` for each touched crate after the task’s tests pass.
- Do not rerun tests after `just fmt` or `just fix -p ...`.
- If any task changes `Cargo.toml` or `Cargo.lock`, run `just bazel-lock-update` and `just bazel-lock-check`.
- If any task changes `codex-rs/app-server-protocol/src/protocol/v2.rs`, run `just write-app-server-schema` and `cargo test -p codex-app-server-protocol`.
- If any task changes `codex-rs/config/src/types.rs` or nested config types, run `just write-config-schema`.
- Ask the user before running workspace-wide `cargo test` because this plan touches `codex-core`.

## Planned File Layout

- Create `codex-rs/product-identity/Cargo.toml`, `codex-rs/product-identity/BUILD.bazel`, and `codex-rs/product-identity/src/lib.rs` to define the narrow fork identity contract: active/legacy home names, env vars, Unix system config roots, Windows admin config roots, macOS managed-preferences domain, release metadata, installer names, and managed-config compatibility roots.
- Modify `codex-rs/Cargo.toml`, consumer `BUILD.bazel` files, and generated lock metadata required by the new `codex-product-identity` crate.
- Modify `codex-rs/utils/home-dir/Cargo.toml` and `codex-rs/utils/home-dir/src/lib.rs` to resolve the active `mcodex` home from the product identity unit, expose legacy-home probing for migration, and keep the caller-facing API centralized in one crate.
- Preserve user-level reusable skills discovery under `~/.agents/skills` and repo `.agents/...`; only product-private caches and deprecated `$CODEX_HOME/skills` compatibility behavior should move with the active product home.
- Modify `codex-rs/core/Cargo.toml`, `codex-rs/core/src/config_loader/mod.rs`, `codex-rs/core/src/config_loader/layer_io.rs`, `codex-rs/core/src/config_loader/macos.rs`, and `codex-rs/core/src/config_loader/tests.rs` so system `config.toml`, `requirements.toml`, macOS managed preferences, and legacy `managed_config.toml` defaults use active `mcodex` identity/roots while legacy upstream values remain available only through explicit backward-compatibility helpers.
- Modify `codex-rs/core-skills/Cargo.toml`, `codex-rs/core-skills/src/loader.rs`, and `codex-rs/core-skills/src/loader_tests.rs` so admin-scoped skills use the active product system root instead of hard-coded `/etc/codex/skills`.
- Modify `codex-rs/tui/src/debug_config.rs` so debug/config rendering and tests report the active `mcodex` system/admin roots and preserve legacy managed-config labels only where the runtime actually loaded a legacy shim.
- Create `codex-rs/core/src/product_identity_migration.rs` to own first-run migration detection, markers, config transform rules, and auth copy orchestration.
- Create `codex-rs/core/src/product_identity_migration_tests.rs` for migration detection, marker, config-transform, and import-boundary coverage.
- Modify `codex-rs/core/src/lib.rs` to export the new product-identity migration module.
- Modify `codex-rs/core/src/personality_migration.rs` only if a small helper extraction is needed for explicit sequencing; keep the existing migration semantics intact.
- Modify `codex-rs/app-server/src/lib.rs` and `codex-rs/tui/src/lib.rs` to run product-identity migration before personality migration and before the rest of startup.
- Modify `codex-rs/state/src/model/account_pool.rs` to add additive startup-status model types and resolution-source enums without changing existing preview semantics.
- Modify `codex-rs/state/src/runtime/account_pool.rs` to populate the richer status output while keeping `preview_account_startup_selection(...)` usable by older call sites.
- Create `codex-rs/account-pool/src/startup_status.rs` to hold the thin adapter that combines state startup facts with resolved config/defaulted policy and pooled-applicability rules.
- Modify `codex-rs/account-pool/src/lib.rs` and, if needed, `codex-rs/account-pool/src/types.rs` to export the startup-status adapter cleanly.
- Modify `codex-rs/core/Cargo.toml`, `codex-rs/app-server/Cargo.toml`, and `codex-rs/tui/Cargo.toml` to depend on `codex-account-pool` if the shared adapter is consumed directly there.
- Modify `codex-rs/core/src/state/service.rs` to build the account-pool manager from the shared startup-status/applicability decision rather than raw `config.accounts` presence.
- Modify `codex-rs/core/tests/suite/account_pool.rs` for state-only startup, policy-only migrated config, and durable-default precedence coverage.
- Modify `codex-rs/app-server/src/account_lease_api.rs` and `codex-rs/app-server/src/codex_message_processor.rs` to replace the current config-only pooled gate with the shared startup-status result.
- Modify `codex-rs/app-server-protocol/src/protocol/v2.rs` only additively to expose any new startup-resolution fields needed by remote startup probing.
- Modify `codex-rs/app-server/tests/suite/v2/account_lease.rs` for additive response fields, migrated-policy gating, and config-default precedence.
- Modify `codex-rs/cli/src/accounts/diagnostics.rs` and `codex-rs/cli/src/accounts/output.rs` to use the shared startup-status result and keep legacy JSON fields stable.
- Modify `codex-rs/cli/src/accounts/registration.rs` to persist startup default selection only in the fresh-home case defined by the spec.
- Modify `codex-rs/cli/src/main.rs` to switch primary binary/help branding to `mcodex` and keep startup/help text aligned with the new product identity.
- Modify `codex-rs/cli/tests/accounts.rs` for config-less startup, additive status JSON, and fresh-home `accounts add --account-pool ...` behavior.
- Modify `codex-rs/tui/src/startup_access.rs` to replace local open-coded state-file probing with the shared startup-status result.
- Modify `codex-rs/tui/src/lib.rs` to adopt the shared startup-status result and updated product-identity migration ordering.
- Modify `codex-rs/tui/src/updates.rs`, `codex-rs/tui/src/update_prompt.rs`, `scripts/install/install.sh`, `scripts/install/install.ps1`, and `scripts/dev/install-local.sh` to point at `mcodex` product identity and home naming.
- Update `docs/config.md` and `docs/install.md` for the new home/env names and first-run migration behavior.

### Task 1A: Establish Product Identity And Home Resolution

**Files:**
- Create: `codex-rs/product-identity/Cargo.toml`
- Create: `codex-rs/product-identity/BUILD.bazel`
- Create: `codex-rs/product-identity/src/lib.rs`
- Modify: `codex-rs/Cargo.toml`
- Modify: `codex-rs/utils/home-dir/Cargo.toml`
- Modify: `codex-rs/utils/home-dir/BUILD.bazel`
- Modify: `codex-rs/utils/home-dir/src/lib.rs`
- Test: `codex-rs/product-identity/src/lib.rs`
- Test: `codex-rs/utils/home-dir/src/lib.rs`

- [ ] **Step 1: Write failing product-identity tests**

Add focused tests in `codex-rs/product-identity/src/lib.rs`:

```rust
#[test]
fn mcodex_identity_defines_active_and_legacy_roots() {
    assert_eq!(MCODEX.product_name, "mcodex");
    assert_eq!(MCODEX.binary_name, "mcodex");
    assert_eq!(MCODEX.default_home_dir_name, ".mcodex");
    assert_eq!(MCODEX.home_env_var, "MCODEX_HOME");
    assert_eq!(MCODEX.legacy_home_env_var, "CODEX_HOME");
    assert_eq!(MCODEX.unix_system_config_root, "/etc/mcodex");
    assert_eq!(MCODEX.legacy_unix_system_config_root, "/etc/codex");
    assert!(MCODEX.windows_admin_config_components.contains(&"Mcodex"));
    assert!(MCODEX.legacy_windows_admin_config_components.contains(&"Codex"));
    assert_eq!(MCODEX.macos_managed_config_domain, "com.vivym.mcodex");
}
```

- [ ] **Step 2: Write failing home-resolution tests**

Add focused tests in `codex-rs/utils/home-dir/src/lib.rs`:

```rust
#[test]
fn find_codex_home_prefers_mcodex_home_env() {
    let temp_home = TempDir::new().expect("temp home");
    let resolved = find_codex_home_from_envs(
        /*active_home_env*/ Some(temp_home.path()),
        /*legacy_home_env*/ None,
    )
    .expect("resolve active home");

    assert_eq!(resolved, expected_absolute(temp_home.path()));
}

#[test]
fn find_codex_home_without_env_uses_dot_mcodex() {
    let resolved = find_codex_home_from_envs(None, None).expect("default home");
    assert!(resolved.as_path().ends_with(".mcodex"));
}

#[test]
fn find_legacy_codex_home_for_migration_prefers_codex_home_env() {
    let legacy_home = TempDir::new().expect("legacy home");
    let resolved = find_legacy_codex_home_for_migration(Some(legacy_home.path()))
        .expect("legacy home");

    assert_eq!(resolved, Some(expected_absolute(legacy_home.path())));
}

#[test]
fn find_codex_home_ignores_codex_home_when_mcodex_home_is_unset() {
    let legacy_home = TempDir::new().expect("legacy home");
    let resolved = find_codex_home_from_envs(
        /*active_home_env*/ None,
        /*legacy_home_env*/ Some(legacy_home.path()),
    )
    .expect("default home");

    assert_ne!(resolved, expected_absolute(legacy_home.path()));
    assert!(resolved.as_path().ends_with(".mcodex"));
}
```

- [ ] **Step 3: Implement the product identity unit**

Create `codex-rs/product-identity/src/lib.rs` with a small data-only API. Keep it free of runtime config parsing and business logic:

```rust
pub struct ProductIdentity {
    pub product_name: &'static str,
    pub binary_name: &'static str,
    pub default_home_dir_name: &'static str,
    pub home_env_var: &'static str,
    pub legacy_binary_name: &'static str,
    pub legacy_home_dir_name: &'static str,
    pub legacy_home_env_var: &'static str,
    pub unix_system_config_root: &'static str,
    pub legacy_unix_system_config_root: &'static str,
    pub windows_admin_config_components: &'static [&'static str],
    pub legacy_windows_admin_config_components: &'static [&'static str],
    pub github_repo_owner: &'static str,
    pub github_repo_name: &'static str,
    pub release_api_url: &'static str,
    pub release_notes_url: &'static str,
    pub installer_dir_name: &'static str,
    pub macos_managed_config_domain: &'static str,
}

pub const MCODEX: ProductIdentity = ProductIdentity {
    product_name: "mcodex",
    binary_name: "mcodex",
    default_home_dir_name: ".mcodex",
    home_env_var: "MCODEX_HOME",
    legacy_binary_name: "codex",
    legacy_home_dir_name: ".codex",
    legacy_home_env_var: "CODEX_HOME",
    unix_system_config_root: "/etc/mcodex",
    legacy_unix_system_config_root: "/etc/codex",
    windows_admin_config_components: &["Mcodex"],
    legacy_windows_admin_config_components: &["OpenAI", "Codex"],
    github_repo_owner: "vivym",
    github_repo_name: "mcodex",
    release_api_url: "https://api.github.com/repos/vivym/mcodex/releases/latest",
    release_notes_url: "https://github.com/vivym/mcodex/releases/latest",
    installer_dir_name: "mcodex",
    macos_managed_config_domain: "com.vivym.mcodex",
};
```

- [ ] **Step 4: Implement active and legacy home helpers**

Update `codex-rs/utils/home-dir/src/lib.rs` so the active API resolves `MCODEX_HOME` / `~/.mcodex`, while a separate helper handles legacy import probing:

```rust
pub fn find_codex_home() -> io::Result<AbsolutePathBuf> {
    find_codex_home_from_env(std::env::var(MCODEX.home_env_var).ok().as_deref())
}

pub fn find_legacy_codex_home_for_migration(
    legacy_home_env: Option<&str>,
) -> io::Result<Option<AbsolutePathBuf>> {
    // prefer CODEX_HOME, else ~/.codex, but require readability/preflight success
}
```

Do not add a normal runtime fallback from `MCODEX_HOME` to `CODEX_HOME`.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cd codex-rs
cargo test -p codex-product-identity
cargo test -p codex-utils-home-dir
```

Expected: PASS. The new tests prove `MCODEX_HOME` is authoritative, legacy probing still sees `CODEX_HOME`, and the active default home is `~/.mcodex`.

- [ ] **Step 6: Format, lint, lock, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-product-identity
just fix -p codex-utils-home-dir
cd ..
just bazel-lock-update
just bazel-lock-check
git add MODULE.bazel.lock codex-rs/Cargo.toml codex-rs/product-identity/Cargo.toml codex-rs/product-identity/BUILD.bazel codex-rs/product-identity/src/lib.rs codex-rs/utils/home-dir/Cargo.toml codex-rs/utils/home-dir/BUILD.bazel codex-rs/utils/home-dir/src/lib.rs
git commit -m "feat(identity): add mcodex product identity roots"
```

### Task 1B: Adopt Product Identity In Config Roots And Skills

**Files:**
- Modify: `codex-rs/core/Cargo.toml`
- Modify: `codex-rs/core/BUILD.bazel`
- Modify: `codex-rs/core/src/config_loader/mod.rs`
- Modify: `codex-rs/core/src/config_loader/layer_io.rs`
- Modify: `codex-rs/core/src/config_loader/macos.rs`
- Modify: `codex-rs/core/src/config_loader/tests.rs`
- Modify: `codex-rs/core-skills/Cargo.toml`
- Modify: `codex-rs/core-skills/BUILD.bazel`
- Modify: `codex-rs/core-skills/src/loader.rs`
- Modify: `codex-rs/core-skills/src/loader_tests.rs`
- Modify: `codex-rs/tui/Cargo.toml`
- Modify: `codex-rs/tui/BUILD.bazel`
- Modify: `codex-rs/tui/src/debug_config.rs`
- Test: `codex-rs/core/src/config_loader/tests.rs`
- Test: `codex-rs/core-skills/src/loader_tests.rs`
- Test: `codex-rs/tui/src/debug_config.rs`

- [ ] **Step 1: Write failing system/admin root tests**

Add targeted tests in `codex-rs/core/src/config_loader/tests.rs` or the closest existing config-loader unit module:

```rust
#[test]
fn system_config_toml_file_uses_active_mcodex_unix_root() {
    assert_eq!(
        unix_system_config_toml_file_for_tests().as_path(),
        Path::new("/etc/mcodex/config.toml")
    );
}

#[test]
fn managed_config_default_path_uses_active_mcodex_unix_root() {
    assert_eq!(
        managed_config_default_path_for_tests().as_path(),
        Path::new("/etc/mcodex/managed_config.toml")
    );
}

#[test]
fn managed_preferences_source_uses_active_mcodex_domain() {
    assert_eq!(
        managed_preferences_requirements_source_for_tests().domain(),
        "com.vivym.mcodex"
    );
}
```

- [ ] **Step 2: Write failing skill-root tests**

Add targeted tests in `codex-rs/core-skills/src/loader_tests.rs`:

```rust
#[test]
fn admin_skills_root_uses_active_mcodex_system_root() {
    assert_eq!(
        admin_skills_root_for_tests().as_path(),
        Path::new("/etc/mcodex/skills")
    );
}
```

Also add or update coverage proving reusable skills are not product-private:

```rust
#[test]
fn user_agents_skills_root_is_preserved_with_mcodex_home() {
    let roots = load_skill_roots_for_test(/*mcodex_home*/ "/tmp/home/.mcodex");
    assert!(roots.iter().any(|root| root.path.ends_with(".agents/skills")));
    assert!(!roots.iter().any(|root| root.path.ends_with(".codex/skills")));
}
```

- [ ] **Step 3: Update debug-config expectations**

Update `codex-rs/tui/src/debug_config.rs` tests so expected rendered paths use `/etc/mcodex/...` and the active Windows admin root, not `/etc/codex/...` or `C:\ProgramData\OpenAI\Codex\...`, except in tests that explicitly exercise a legacy managed-config shim.

- [ ] **Step 4: Implement active identity roots in config and skills**

Update the config-loader, macOS managed-preferences loader, managed-config loader, admin-skills loader, and debug-config code to consume `codex-product-identity` rather than hard-coded upstream roots.

Do not rename repo-local `.agents/skills`, repo-local `.agents/plugins`, or user-global `~/.agents/skills`. Only product-private roots and admin/system roots should move to `mcodex`.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core config_loader -- --nocapture
cargo test -p codex-core-skills
cargo test -p codex-tui debug_config -- --nocapture
```

Expected: PASS. Active system/admin roots resolve to `mcodex`, legacy upstream roots are not loaded by default, admin skills use `/etc/mcodex/skills`, and user-global `~/.agents/skills` discovery remains unchanged.

- [ ] **Step 6: Format, lint, lock, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-core
just fix -p codex-core-skills
just fix -p codex-tui
cd ..
just bazel-lock-update
just bazel-lock-check
git add MODULE.bazel.lock codex-rs/core/Cargo.toml codex-rs/core/BUILD.bazel codex-rs/core/src/config_loader/mod.rs codex-rs/core/src/config_loader/layer_io.rs codex-rs/core/src/config_loader/macos.rs codex-rs/core/src/config_loader/tests.rs codex-rs/core-skills/Cargo.toml codex-rs/core-skills/BUILD.bazel codex-rs/core-skills/src/loader.rs codex-rs/core-skills/src/loader_tests.rs codex-rs/tui/Cargo.toml codex-rs/tui/BUILD.bazel codex-rs/tui/src/debug_config.rs
git commit -m "feat(identity): route config roots through mcodex identity"
```

### Task 1C: Add Product Identity Migration Service

**Files:**
- Create: `codex-rs/core/src/product_identity_migration.rs`
- Create: `codex-rs/core/src/product_identity_migration_tests.rs`
- Modify: `codex-rs/core/src/lib.rs`
- Test: `codex-rs/core/src/product_identity_migration_tests.rs`

- [ ] **Step 1: Write failing product-migration tests**

Add targeted tests in `codex-rs/core/src/product_identity_migration_tests.rs`:

```rust
#[tokio::test]
async fn config_transform_drops_startup_selection_fields() -> Result<()> {
    let transformed = transform_imported_config(parse_toml(r#"
[accounts]
default_pool = "team-main"
allocation_mode = "exclusive"
"#)?)?;

    assert_eq!(transformed.accounts.as_ref().and_then(|accounts| accounts.default_pool.clone()), None);
    assert_eq!(
        transformed.accounts.as_ref().and_then(|accounts| accounts.allocation_mode.clone()),
        Some(AccountAllocationModeToml::Exclusive)
    );
    Ok(())
}

#[tokio::test]
async fn auth_import_failure_is_reported_as_warning_without_blocking_startup() -> Result<()> {
    let outcome = maybe_migrate_product_identity(/* failing auth copy */).await?;
    assert_eq!(outcome.status, ProductIdentityMigrationStatus::ImportedWithWarnings);
    assert!(matches!(outcome.auth_import, MigrationImportOutcome::Failed { .. }));
    Ok(())
}

#[tokio::test]
async fn migration_does_not_import_legacy_skill_or_plugin_caches_by_default() -> Result<()> {
    // seed legacy ~/.codex/skills and ~/.codex/plugins/cache
    // import config/auth
    // expect no copied skills or plugin cache under ~/.mcodex
    Ok(())
}
```

- [ ] **Step 2: Implement the shared migration service**

Create `codex-rs/core/src/product_identity_migration.rs` with a small, testable service:

```rust
pub const PRODUCT_IDENTITY_MIGRATION_FILENAME: &str = ".product_identity_migration";

pub enum MigrationImportOutcome {
    NotAttempted,
    Imported,
    Failed { warning: String },
}

pub enum ProductIdentityMigrationStatus {
    SkippedMarker,
    SkippedNoLegacyHome,
    SkippedUnreadableLegacyHome,
    SkippedByUser,
    Imported,
    ImportedWithWarnings,
}

pub struct ProductIdentityMigrationOutcome {
    pub status: ProductIdentityMigrationStatus,
    pub config_import: MigrationImportOutcome,
    pub auth_import: MigrationImportOutcome,
    pub marker_warning: Option<String>,
}

pub async fn maybe_migrate_product_identity(
    mcodex_home: &Path,
    ui: &mut dyn ProductIdentityMigrationUi,
) -> io::Result<ProductIdentityMigrationOutcome> {
    // detect legacy home, prompt, transform config, copy auth, write marker
}
```

The config transform must drop startup-selection-bearing fields whose meaning depends on non-imported runtime state. It should preserve only fields that are explicitly allowed by the account-pool startup-selection spec, such as policy fields that do not imply local pooled runtime state. Do not expand this task into a full pooled-runtime redesign. Marker-write failures and config/auth import failures must be represented as warnings in the outcome instead of bricking startup.

Do not import legacy skills, plugin caches, marketplace caches, logs, session history, or SQLite state in this task. Keep `~/.agents/skills` available through normal skill discovery instead.

- [ ] **Step 3: Run targeted tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core product_identity_migration -- --nocapture
```

Expected: PASS. Migration prompts and imports config/auth only, records its own marker, drops startup-selection-bearing config, and does not copy legacy skills/plugins/runtime caches.

- [ ] **Step 4: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-core
git add core/src/lib.rs core/src/product_identity_migration.rs core/src/product_identity_migration_tests.rs
git commit -m "feat(startup): add mcodex identity migration service"
```

### Task 2: Compose Product Migration With Personality Migration In Startup Paths

**Files:**
- Modify: `codex-rs/app-server/src/lib.rs`
- Modify: `codex-rs/tui/src/lib.rs`
- Modify: `codex-rs/core/src/personality_migration.rs` only if a small sequencing helper is needed
- Test: `codex-rs/core/tests/suite/personality_migration.rs`
- Test: `codex-rs/core/src/product_identity_migration_tests.rs`

- [ ] **Step 1: Write failing sequencing tests**

Extend `codex-rs/core/tests/suite/personality_migration.rs` or the new product-migration tests:

```rust
#[tokio::test]
async fn product_migration_marker_does_not_suppress_personality_migration() -> Result<()> {
    // seed product marker only
    // expect personality migration still runs if its own conditions are met
}

#[tokio::test]
async fn personality_marker_does_not_suppress_product_migration() -> Result<()> {
    // seed personality marker only
    // expect product migration still prompts/imports when legacy home is present
}
```

- [ ] **Step 2: Add a shared startup-migration orchestration helper**

In `codex-rs/core/src/product_identity_migration.rs`, add a narrow orchestration entry point:

```rust
pub struct StartupMigrationOutcome {
    pub product_identity: ProductIdentityMigrationStatus,
    pub personality: PersonalityMigrationStatus,
}

pub async fn run_startup_migrations(
    codex_home: &Path,
    config_toml: &ConfigToml,
    ui: &mut dyn ProductIdentityMigrationUi,
) -> io::Result<StartupMigrationOutcome> {
    let product_identity = maybe_migrate_product_identity(codex_home, ui).await?;
    let personality = maybe_migrate_personality(codex_home, config_toml).await?;
    Ok(StartupMigrationOutcome {
        product_identity,
        personality,
    })
}
```

- [ ] **Step 3: Wire TUI startup to prompt before personality migration**

Update `codex-rs/tui/src/lib.rs` so startup:

1. resolves `mcodex` home
2. runs the product-identity migration prompt/import flow
3. only then runs personality migration
4. continues with normal config/login/onboarding startup

Keep the migration prompt line-oriented and pre-alt-screen in this slice so no new ratatui onboarding surface is required.

- [ ] **Step 4: Wire app-server startup to the same ordering**

Update `codex-rs/app-server/src/lib.rs` to invoke the same startup-migration orchestration before the existing personality-migration call. Use the same disclosure text and keep markers separate. If app-server startup is noninteractive, fail clearly rather than silently half-running migration logic.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p codex-core personality_migration -- --nocapture
cargo test -p codex-core product_identity_migration -- --nocapture
```

Expected: PASS. Ordering is explicit, and each marker suppresses only its own migration.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-core
just fix -p codex-tui
just fix -p codex-app-server
git add core/src/personality_migration.rs core/tests/suite/personality_migration.rs app-server/src/lib.rs tui/src/lib.rs core/src/product_identity_migration.rs core/src/product_identity_migration_tests.rs
git commit -m "feat(startup): order product and personality migrations"
```

### Task 3: Add The Shared Additive Startup-Status Surface

**Files:**
- Modify: `codex-rs/state/src/model/account_pool.rs`
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Create: `codex-rs/account-pool/src/startup_status.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Test: `codex-rs/state/src/runtime/account_pool.rs`
- Test: `codex-rs/account-pool/tests/lease_lifecycle.rs`

- [ ] **Step 1: Write failing state/account-pool tests**

Add tests covering resolution source and applicability:

```rust
#[tokio::test]
async fn startup_status_prefers_configured_default_over_persisted_default() -> Result<()> {
    let runtime = test_runtime().await;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("persisted-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;

    let status = runtime
        .read_account_startup_status(Some("configured-main"))
        .await?;

    assert_eq!(status.configured_default_pool_id.as_deref(), Some("configured-main"));
    assert_eq!(status.persisted_default_pool_id.as_deref(), Some("persisted-main"));
    assert_eq!(status.effective_pool_resolution_source, EffectivePoolResolutionSource::ConfigDefault);
    Ok(())
}

#[tokio::test]
async fn startup_applicability_rejects_policy_only_migrated_config_without_membership() -> Result<()> {
    let status = shared_startup_status_adapter(/*config_default_pool*/ None, /*has_membership*/ false, /*policy_only_config*/ true)?;
    assert_eq!(status.pooled_applicable, false);
    Ok(())
}
```

- [ ] **Step 2: Add additive model types in `codex-state`**

Extend `codex-rs/state/src/model/account_pool.rs` additively:

```rust
pub enum EffectivePoolResolutionSource {
    Override,
    ConfigDefault,
    PersistedSelection,
    None,
}

pub struct AccountStartupStatus {
    pub preview: AccountStartupSelectionPreview,
    pub configured_default_pool_id: Option<String>,
    pub persisted_default_pool_id: Option<String>,
    pub effective_pool_resolution_source: EffectivePoolResolutionSource,
}
```

Do not change existing `AccountStartupSelectionPreview` field meanings.

- [ ] **Step 3: Extend the state runtime with a companion status reader**

Implement a companion API in `codex-rs/state/src/runtime/account_pool.rs`:

```rust
pub async fn read_account_startup_status(
    &self,
    configured_default_pool_id: Option<&str>,
) -> anyhow::Result<AccountStartupStatus> {
    // reuse preview_account_startup_selection semantics, then annotate configured/persisted source
}
```

- [ ] **Step 4: Add the thin `codex-account-pool` adapter**

Create `codex-rs/account-pool/src/startup_status.rs`:

```rust
pub struct SharedStartupStatus {
    pub startup: AccountStartupStatus,
    pub pooled_applicable: bool,
}

pub async fn read_shared_startup_status<B: AccountPoolExecutionBackend>(
    backend: &B,
    configured_default_pool_id: Option<&str>,
    explicit_override_pool_id: Option<&str>,
) -> anyhow::Result<SharedStartupStatus> {
    // state-backed facts + explicit override/config intent + pooled-applicability rules
}
```

The adapter must remain thin: no duplicated membership resolver, no second precedence tree, no silent contract rewrite.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p codex-state account_pool -- --nocapture
cargo test -p codex-account-pool
```

Expected: PASS. The status surface is additive, and pooled applicability differentiates local explicit intent from migrated policy-only config.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-state
just fix -p codex-account-pool
git add state/src/model/account_pool.rs state/src/runtime/account_pool.rs account-pool/src/startup_status.rs account-pool/src/lib.rs
git commit -m "feat(account-pool): add shared startup status adapter"
```

### Task 4: Adopt Shared Startup Status In Core And App-Server

**Files:**
- Modify: `codex-rs/core/Cargo.toml`
- Modify: `codex-rs/app-server/Cargo.toml`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/tests/suite/account_pool.rs`
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_lease.rs`
- Generate: `codex-rs/app-server-protocol/schema` via `just write-app-server-schema`

- [ ] **Step 1: Write failing core and app-server tests**

Add focused tests:

```rust
#[tokio::test]
async fn pooled_manager_builds_for_state_only_home_with_local_membership() -> Result<()> {
    // no config.accounts
    // seeded membership in local state
    // expect SessionServices::build_account_pool_manager(...) to return Some(...)
}

#[tokio::test]
async fn policy_only_migrated_config_does_not_enable_account_lease_runtime() -> Result<()> {
    // config has accounts policy but no default_pool and no membership
    // expect accountLease/read to stay empty and accountLease/resume to fail closed gracefully
}

#[tokio::test]
async fn account_lease_read_adds_resolution_fields_without_changing_legacy_fields() -> Result<()> {
    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(response.switch_reason.as_deref(), Some("automaticAccountSelected"));
    assert_eq!(response.effective_pool_resolution_source.as_deref(), Some("configDefault"));
    Ok(())
}
```

- [ ] **Step 2: Add workspace dependencies and refresh Bazel locks**

If `codex-core` and `codex-app-server` consume `codex-account-pool` directly, update:

```toml
codex-account-pool = { workspace = true }
```

Then run:

```bash
cd codex-rs
just bazel-lock-update
just bazel-lock-check
```

- [ ] **Step 3: Replace config-only gating in `codex-core`**

Update `codex-rs/core/src/state/service.rs` so manager construction uses the shared startup-status/applicability result rather than `accounts?` short-circuiting:

```rust
let shared_status = read_shared_startup_status(/*...*/).await?;
if !shared_status.pooled_applicable {
    return None;
}
```

Preserve `account_pool_manager: Option<_>` as the top-level non-pooled gate.

- [ ] **Step 4: Replace config-only gating in app-server**

Update `codex-rs/app-server/src/account_lease_api.rs` and `codex-rs/app-server/src/codex_message_processor.rs` so:

- policy-only migrated config does not count as pooled mode
- state-only local homes do count once local membership/default intent exists
- `AccountLeaseReadResponse` gains only additive fields such as:

```rust
pub effective_pool_resolution_source: Option<String>,
pub configured_default_pool_id: Option<String>,
pub persisted_default_pool_id: Option<String>,
```

- [ ] **Step 5: Regenerate protocol schema and run targeted tests**

Run:

```bash
cd codex-rs
just write-app-server-schema
cargo test -p codex-app-server-protocol
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-app-server account_lease -- --nocapture
```

Expected: PASS. Public protocol changes stay additive, and pooled gating now matches the shared startup-status rules.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-core
just fix -p codex-app-server-protocol
just fix -p codex-app-server
git add core/Cargo.toml app-server/Cargo.toml core/src/state/service.rs core/tests/suite/account_pool.rs app-server/src/account_lease_api.rs app-server/src/codex_message_processor.rs app-server-protocol/src/protocol/v2.rs app-server/tests/suite/v2/account_lease.rs
git commit -m "feat(runtime): share pooled startup status across core and app-server"
```

### Task 5: Update CLI Diagnostics And Registration Semantics

**Files:**
- Modify: `codex-rs/cli/src/accounts/diagnostics.rs`
- Modify: `codex-rs/cli/src/accounts/output.rs`
- Modify: `codex-rs/cli/src/accounts/registration.rs`
- Modify: `codex-rs/cli/src/accounts/mod.rs` only if new plumbing is required
- Test: `codex-rs/cli/tests/accounts.rs`

- [ ] **Step 1: Write failing CLI tests**

Add focused tests in `codex-rs/cli/tests/accounts.rs`:

```rust
#[tokio::test]
async fn status_reports_registered_pools_without_config_toml() -> Result<()> {
    let home = prepared_legacy_auth_only_home().await?;
    seed_state(home.path()).await?;

    let output = run_codex(&home, &["accounts", "status", "--json"]).await?;
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;

    assert_eq!(json["configuredPoolCount"], 0);
    assert_eq!(json["registeredPoolCount"], 1);
    assert_eq!(json["effectivePoolResolutionSource"], "persistedSelection");
    Ok(())
}

#[tokio::test]
async fn add_account_pool_persists_default_only_when_no_durable_default_exists() -> Result<()> {
    // no config default, no persisted default => add --account-pool persists default_pool_id
}

#[tokio::test]
async fn add_account_pool_does_not_override_existing_config_default() -> Result<()> {
    // config default present => registration joins pool but does not rewrite durable selection
}
```

- [ ] **Step 2: Switch diagnostics to the shared status adapter**

Update `codex-rs/cli/src/accounts/diagnostics.rs` to read the shared startup status once and fan out from that:

```rust
pub(crate) struct AccountsStatusDiagnostic {
    pub startup: SharedStartupStatus,
    pub configured_pool_count: usize,
    pub registered_pool_count: usize,
    // ...
}
```

- [ ] **Step 3: Preserve legacy JSON while adding resolution fields**

Update `codex-rs/cli/src/accounts/output.rs` so:

- `effectivePoolSource` keeps its current account-source meaning
- `configuredPoolCount` keeps its current config-policy meaning
- new fields such as `registeredPoolCount`, `configuredDefaultPoolId`, `persistedDefaultPoolId`, and `effectivePoolResolutionSource` are added additively

- [ ] **Step 4: Fix `accounts add --account-pool ...` persistence rules**

Update `codex-rs/cli/src/accounts/registration.rs`:

```rust
if configured_default_pool_id.is_none() && persisted_default_pool_id.is_none() {
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some(target_pool_id.clone()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;
}
```

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p codex-cli accounts -- --nocapture
```

Expected: PASS. `accounts status` now tells the truth about config vs runtime, and fresh-home registration establishes local startup selection only when the spec allows it.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
git add cli/src/accounts/diagnostics.rs cli/src/accounts/output.rs cli/src/accounts/registration.rs cli/src/accounts/mod.rs cli/tests/accounts.rs
git commit -m "feat(cli): report pooled startup status consistently"
```

### Task 6: Update TUI Startup Probing To Use Shared Startup Status

**Files:**
- Modify: `codex-rs/tui/Cargo.toml`
- Modify: `codex-rs/tui/src/startup_access.rs`
- Modify: `codex-rs/tui/src/lib.rs`
- Test: `codex-rs/tui/src/startup_access.rs`

- [ ] **Step 1: Write failing TUI startup-access tests**

Extend `codex-rs/tui/src/startup_access.rs` tests:

```rust
#[tokio::test]
async fn local_probe_uses_state_only_membership_without_config_accounts() -> Result<()> {
    // seed state-only home with membership
    // expect StartupProbe::PooledAvailable { remote: false }
}

#[tokio::test]
async fn local_probe_rejects_policy_only_migrated_config_without_membership() -> Result<()> {
    // no local membership, imported policy-only config
    // expect StartupProbe::Unavailable
}

#[tokio::test]
async fn local_probe_does_not_require_preexisting_sqlite_file_for_config_default() -> Result<()> {
    // config default present on fresh home
    // expect local runtime init to happen and probe to use shared status instead of state_path.exists()
}
```

- [ ] **Step 2: Add any required workspace dependency**

If `codex-tui` consumes `codex-account-pool` directly, add it to `codex-rs/tui/Cargo.toml`, then rerun:

```bash
cd codex-rs
just bazel-lock-update
just bazel-lock-check
```

- [ ] **Step 3: Replace the local file-existence gate**

Update `codex-rs/tui/src/startup_access.rs` to remove:

```rust
if configured_default_pool_id(config).is_none() && !state_path.exists() {
    return Ok(StartupProbe::Unavailable);
}
```

and instead call the shared startup-status/applicability adapter after initializing the local state runtime for the active product home.

- [ ] **Step 4: Keep existing notice behavior intact**

Only the probe source changes in this slice. Preserve:

- `NeedsLogin`
- `PooledOnlyNotice`
- `PooledAccessPausedNotice`
- hidden-notice handling

Do not redesign onboarding.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p codex-tui startup_access -- --nocapture
```

Expected: PASS. The local TUI probe no longer falls back to the login wall for valid pooled state/defaults.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-tui
git add tui/Cargo.toml tui/src/startup_access.rs tui/src/lib.rs
git commit -m "feat(tui): use shared pooled startup status"
```

### Task 7: Update Install/Update Identity Edges And Documentation

**Files:**
- Modify: `codex-rs/cli/Cargo.toml`
- Modify: `codex-rs/cli/BUILD.bazel`
- Modify: `codex-rs/cli/src/main.rs`
- Modify: `scripts/install/install.sh`
- Modify: `scripts/install/install.ps1`
- Modify: `scripts/dev/install-local.sh`
- Modify: `codex-rs/tui/src/updates.rs`
- Modify: `codex-rs/tui/src/update_prompt.rs`
- Modify: `docs/config.md`
- Modify: `docs/install.md`

- [ ] **Step 1: Write or extend the smallest available tests/checks**

If no automated coverage exists for the installer/update paths, at minimum add assertions where there is existing unit coverage and otherwise document manual smoke checks in the commit message and PR notes. For any existing unit tests around update URLs or product naming, extend them first.

- [ ] **Step 2: Switch install/update identity**

Update scripts and update surfaces so they no longer point at upstream `openai/codex` or `.codex` defaults:

```bash
# examples to replace
https://api.github.com/repos/openai/codex/releases/latest
~/.codex
CODEX_HOME
```

with fork-appropriate product identity and `MCODEX_HOME`/`~/.mcodex`.

Where a Rust surface needs product names, release URLs, or installer directory names, consume `codex_product_identity::MCODEX` instead of introducing new hard-coded fork strings. Keep shell/PowerShell scripts aligned with the same values from Task 1.

Update `codex-rs/cli/src/main.rs` in the same task so the clap binary/help identity also moves to `mcodex`, for example:

```rust
#[command(bin_name = "mcodex")]
struct Cli {
    // ...
}
```

- [ ] **Step 3: Refresh user-facing docs**

Update `docs/config.md` and `docs/install.md` so they describe:

- `MCODEX_HOME` as the active runtime override
- `~/.mcodex` as the default home
- first-run migration from legacy `CODEX_HOME`/`~/.codex`
- the fact that pooled startup selection is re-established locally after import

- [ ] **Step 4: Run targeted checks**

Run:

```bash
cd codex-rs
cargo test -p codex-cli -- --nocapture
cargo test -p codex-tui updates -- --nocapture
cd ..
rg -n "CODEX_HOME|~/.codex|openai/codex|bin_name = \\\"codex\\\"" scripts/install codex-rs/tui/src codex-rs/cli/src/main.rs docs/config.md docs/install.md
rg -n "/etc/codex|ProgramData\\\\OpenAI\\\\Codex|com\\.openai\\.codex|managed_config" codex-rs/core/src/config_loader codex-rs/core-skills/src codex-rs/tui/src/debug_config.rs docs/config.md docs/install.md
```

Expected: only legacy-migration documentation, intentionally preserved upstream references, and explicitly named legacy managed-config compatibility shims remain.

Review the grep output against this allowlist instead of mechanically deleting every hit:

- `codex-product-identity` active/legacy constants and tests
- product-identity migration code, tests, and docs that explicitly refer to legacy import sources
- managed-config compatibility shims that intentionally probe legacy upstream paths
- docs explaining how `mcodex` imports from `CODEX_HOME` / `~/.codex`
- tests whose names or fixtures explicitly assert legacy behavior

Any hit outside that allowlist needs either a code change or a short inline rationale in the implementation notes.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
just fix -p codex-tui
cd ..
just bazel-lock-update
just bazel-lock-check
git add MODULE.bazel.lock codex-rs/cli/Cargo.toml codex-rs/cli/BUILD.bazel codex-rs/cli/src/main.rs scripts/install/install.sh scripts/install/install.ps1 scripts/dev/install-local.sh codex-rs/tui/src/updates.rs codex-rs/tui/src/update_prompt.rs docs/config.md docs/install.md
git commit -m "docs: switch install and update surfaces to mcodex"
```

### Task 8: Final Validation And Handoff

**Files:**
- Review only; no new source files expected unless a previous task exposed a gap

- [ ] **Step 1: Run the targeted crate suites one final time**

Run:

```bash
cd codex-rs
cargo test -p codex-product-identity
cargo test -p codex-utils-home-dir
cargo test -p codex-core config_loader -- --nocapture
cargo test -p codex-core product_identity_migration -- --nocapture
cargo test -p codex-core personality_migration -- --nocapture
cargo test -p codex-core-skills
cargo test -p codex-state account_pool -- --nocapture
cargo test -p codex-account-pool
cargo test -p codex-cli accounts -- --nocapture
cargo test -p codex-app-server-protocol
cargo test -p codex-app-server account_lease -- --nocapture
cargo test -p codex-tui debug_config -- --nocapture
cargo test -p codex-tui startup_access -- --nocapture
```

Expected: PASS across all targeted suites.

- [ ] **Step 2: Run required generators/checkers if touched**

Run only if applicable:

```bash
cd codex-rs
just write-app-server-schema
just write-config-schema
just bazel-lock-check
```

- [ ] **Step 3: Ask before any workspace-wide test run**

If shared-core risk still justifies broader coverage, stop here and ask the user before running `cargo test` for the whole workspace.

- [ ] **Step 4: Summarize manual smoke checks**

Verify manually:

1. fresh `mcodex` home with `accounts add --account-pool main-pool` enters pooled startup without manual config edits
2. migrated home from legacy `CODEX_HOME` or `~/.codex` prompts once, discloses that pooled selection is not imported, and writes separate migration markers
3. `accounts status --json` preserves legacy fields while adding resolution fields
4. TUI no longer shows the login wall for valid pooled-only startup access
5. normal runtime and debug config point at `mcodex` system/admin roots and managed-preferences domain, while `/etc/codex/...`, `%ProgramData%\OpenAI\Codex\...`, and `com.openai.codex` appear only in explicit legacy migration or managed-config compatibility paths

- [ ] **Step 5: Final commit or fixup commit**

```bash
git status --short
git add -A
git commit -m "chore: finish mcodex pooled startup and identity rollout"
```

Only create this final commit if the branch still contains uncommitted finishing changes after the task commits above.
