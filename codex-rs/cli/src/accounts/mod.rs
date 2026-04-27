mod default_pool;
mod diagnostics;
mod mutate;
mod observability;
mod observability_output;
mod observability_types;
mod output;
mod registration;

use anyhow::Context;
use clap::Args;
use clap::Parser;
use clap::ValueEnum;
use codex_core::config::Config;
use codex_product_identity::MCODEX;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::StateRuntime;
use codex_state::state_db_path;
use codex_utils_cli::CliConfigOverrides;
use default_pool::clear_default_pool;
use default_pool::reject_process_local_override;
use default_pool::set_default_pool;
use diagnostics::read_current_diagnostic;
use diagnostics::read_status_diagnostic;
use mutate::assign_account_pool;
use mutate::list_account_pools;
use mutate::list_accounts;
use mutate::remove_account;
use mutate::set_account_enabled;
use observability::read_pool_diagnostics;
use observability::read_pool_events;
use observability::read_pool_show;
use observability::resolve_target_pool;
use observability_output::print_diagnostics_json;
use observability_output::print_diagnostics_text;
use observability_output::print_events_json;
use observability_output::print_events_text;
use observability_output::print_pool_show_json;
use observability_output::print_pool_show_text;
use output::print_current_json;
use output::print_current_text;
use output::print_status_json;
use output::print_status_text;
use registration::add_chatgpt_account;
use registration::api_key_add_is_unsupported;
use registration::import_legacy_account;
use std::sync::Arc;

#[derive(Debug, Parser)]
pub struct AccountsCommand {
    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    #[arg(long = "account-pool", value_name = "POOL_ID")]
    pub account_pool: Option<String>,

    #[command(subcommand)]
    pub subcommand: AccountsSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum AccountsSubcommand {
    Add(AddAccountCommand),
    ImportLegacy(ImportLegacyCommand),
    Enable(AccountToggleCommand),
    Disable(AccountToggleCommand),
    Remove(RemoveAccountCommand),
    List,
    Pool(PoolCommand),
    Diagnostics(AccountsDiagnosticsCommand),
    Events(AccountsEventsCommand),
    Current(CurrentAccountCommand),
    Status(StatusAccountCommand),
    Resume,
    Switch(SwitchAccountCommand),
}

#[derive(Debug, Args)]
pub struct AddAccountCommand {
    #[command(subcommand)]
    pub subcommand: Option<AddAccountSubcommand>,
}

#[derive(Debug, clap::Subcommand)]
pub enum AddAccountSubcommand {
    Chatgpt(AddChatgptAccountCommand),
    ApiKey,
}

#[derive(Debug, Args)]
pub struct AddChatgptAccountCommand {
    #[arg(long = "device-auth", default_value_t = false)]
    pub device_auth: bool,
}

#[derive(Debug, Args)]
pub struct ImportLegacyCommand {
    #[arg(long = "pool", value_name = "POOL_ID")]
    pub pool: Option<String>,
}

#[derive(Debug, Args)]
pub struct CurrentAccountCommand {
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AccountToggleCommand {
    #[arg(value_name = "ACCOUNT_ID")]
    pub account_id: String,
}

#[derive(Debug, Args)]
pub struct RemoveAccountCommand {
    #[arg(value_name = "ACCOUNT_ID")]
    pub account_id: String,
}

#[derive(Debug, Args)]
pub struct StatusAccountCommand {
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct SwitchAccountCommand {
    #[arg(value_name = "ACCOUNT_ID")]
    pub account_id: String,
}

#[derive(Debug, Args)]
pub struct PoolCommand {
    #[command(subcommand)]
    pub subcommand: PoolSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum PoolSubcommand {
    List,
    Assign(PoolAssignCommand),
    Default(PoolDefaultCommand),
    Show(PoolShowCommand),
}

#[derive(Debug, Args)]
pub struct PoolAssignCommand {
    #[arg(value_name = "ACCOUNT_ID")]
    pub account_id: String,

    #[arg(value_name = "POOL_ID")]
    pub pool_id: String,
}

#[derive(Debug, Args)]
pub struct PoolDefaultCommand {
    #[command(subcommand)]
    pub subcommand: PoolDefaultSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum PoolDefaultSubcommand {
    Set(PoolDefaultSetCommand),
    Clear,
}

#[derive(Debug, Args)]
pub struct PoolDefaultSetCommand {
    #[arg(value_name = "POOL_ID")]
    pub pool_id: String,
}

#[derive(Debug, Args)]
pub struct PoolShowCommand {
    #[arg(long = "pool", value_name = "POOL_ID")]
    pub pool: Option<String>,

    #[arg(long = "limit")]
    pub limit: Option<u32>,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,

    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AccountsDiagnosticsCommand {
    #[arg(long = "pool", value_name = "POOL_ID")]
    pub pool: Option<String>,

    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AccountsEventsCommand {
    #[arg(long = "pool", value_name = "POOL_ID")]
    pub pool: Option<String>,

    #[arg(long = "account", value_name = "ACCOUNT_ID")]
    pub account: Option<String>,

    #[arg(long = "type", value_enum)]
    pub types: Vec<AccountsEventTypeFilter>,

    #[arg(long = "limit")]
    pub limit: Option<u32>,

    #[arg(long = "cursor")]
    pub cursor: Option<String>,

    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "camelCase")]
pub enum AccountsEventTypeFilter {
    LeaseAcquired,
    LeaseRenewed,
    LeaseReleased,
    LeaseAcquireFailed,
    ProactiveSwitchSelected,
    ProactiveSwitchSuppressed,
    QuotaObserved,
    QuotaNearExhausted,
    QuotaExhausted,
    AccountPaused,
    AccountResumed,
    AccountDrainingStarted,
    AccountDrainingCleared,
    AuthFailed,
    CooldownStarted,
    CooldownCleared,
}

pub async fn run_accounts(command: AccountsCommand) -> ! {
    match run_accounts_impl(command).await {
        Ok(()) => std::process::exit(0),
        Err(err) => {
            eprintln!("Error managing accounts: {err}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn suppress_pooled_startup_selection_if_configured(
    config: &Config,
) -> anyhow::Result<bool> {
    let config_has_accounts = config.accounts.as_ref().is_some_and(|accounts| {
        accounts.default_pool.is_some()
            || accounts
                .pools
                .as_ref()
                .is_some_and(|pools| !pools.is_empty())
    });
    let state_path = state_db_path(config.sqlite_home.as_path());
    if !config_has_accounts && !tokio::fs::try_exists(&state_path).await? {
        return Ok(false);
    }

    let runtime = StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
        .await
        .context("initialize account startup selection state")?;
    let selection = runtime
        .read_account_startup_selection()
        .await
        .context("read account startup selection")?;

    let has_startup_selection = selection.default_pool_id.is_some()
        || selection.preferred_account_id.is_some()
        || selection.suppressed;
    if !config_has_accounts && !has_startup_selection {
        return Ok(false);
    }

    if !selection.suppressed {
        runtime
            .write_account_startup_selection(AccountStartupSelectionUpdate {
                default_pool_id: selection.default_pool_id,
                preferred_account_id: selection.preferred_account_id,
                suppressed: true,
            })
            .await
            .context("write suppressed account startup selection")?;
    }

    Ok(true)
}

async fn run_accounts_impl(command: AccountsCommand) -> anyhow::Result<()> {
    let AccountsCommand {
        config_overrides,
        account_pool,
        subcommand,
    } = command;
    if let AccountsSubcommand::Add(command) = &subcommand
        && matches!(command.subcommand, Some(AddAccountSubcommand::ApiKey))
    {
        return api_key_add_is_unsupported().map(|_| ());
    }
    if let AccountsSubcommand::Pool(PoolCommand {
        subcommand: PoolSubcommand::Default(_),
    }) = &subcommand
    {
        reject_process_local_override(account_pool.as_deref())?;
    }

    let cli_overrides = config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    let config = Config::load_with_cli_overrides(cli_overrides).await?;

    if matches!(subcommand, AccountsSubcommand::Add(_)) && account_pool.is_none() {
        let config_has_accounts = config.accounts.as_ref().is_some_and(|accounts| {
            accounts.default_pool.is_some()
                || accounts
                    .pools
                    .as_ref()
                    .is_some_and(|pools| !pools.is_empty())
        });
        let state_path = state_db_path(config.sqlite_home.as_path());
        if !config_has_accounts && !tokio::fs::try_exists(&state_path).await? {
            anyhow::bail!(
                "no account pool is configured; pass `--account-pool <POOL_ID>` or configure a pool before running `{} accounts add chatgpt`",
                MCODEX.binary_name
            );
        }
    }

    if matches!(subcommand, AccountsSubcommand::List) {
        let config_has_accounts = config.accounts.as_ref().is_some_and(|accounts| {
            accounts.default_pool.is_some()
                || accounts
                    .pools
                    .as_ref()
                    .is_some_and(|pools| !pools.is_empty())
        });
        let state_path = state_db_path(config.sqlite_home.as_path());
        if !config_has_accounts && !tokio::fs::try_exists(&state_path).await? {
            return Ok(());
        }
    }

    match subcommand {
        AccountsSubcommand::Pool(PoolCommand {
            subcommand: PoolSubcommand::Show(command),
        }) => {
            validate_explicit_observability_target(
                command.pool.as_deref(),
                account_pool.as_deref(),
            )?;
            let runtime = Arc::new(
                StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                    .await
                    .context("initialize account startup selection state")?,
            );
            let view = read_pool_show(&runtime, &config, account_pool.as_deref(), &command).await?;
            if command.json {
                print_pool_show_json(&view)?;
            } else {
                print_pool_show_text(&view);
            }
            Ok(())
        }
        AccountsSubcommand::Diagnostics(command) => {
            validate_explicit_observability_target(
                command.pool.as_deref(),
                account_pool.as_deref(),
            )?;
            let runtime = Arc::new(
                StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                    .await
                    .context("initialize account startup selection state")?,
            );
            let view = read_pool_diagnostics(
                &runtime,
                &config,
                account_pool.as_deref(),
                command.pool.as_deref(),
            )
            .await?;
            if command.json {
                print_diagnostics_json(&view)?;
            } else {
                print_diagnostics_text(&view);
            }
            Ok(())
        }
        AccountsSubcommand::Events(command) => {
            validate_explicit_observability_target(
                command.pool.as_deref(),
                account_pool.as_deref(),
            )?;
            let runtime = Arc::new(
                StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                    .await
                    .context("initialize account startup selection state")?,
            );
            let view =
                read_pool_events(&runtime, &config, account_pool.as_deref(), &command).await?;
            if command.json {
                print_events_json(&view)?;
            } else {
                print_events_text(&view);
            }
            Ok(())
        }
        subcommand => {
            let runtime = Arc::new(
                StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                    .await
                    .context("initialize account startup selection state")?,
            );

            match subcommand {
                AccountsSubcommand::Add(command) => {
                    let registered = match command.subcommand {
                        None => {
                            add_chatgpt_account(&runtime, &config, account_pool.as_deref(), false)
                                .await?
                        }
                        Some(AddAccountSubcommand::Chatgpt(command)) => {
                            add_chatgpt_account(
                                &runtime,
                                &config,
                                account_pool.as_deref(),
                                command.device_auth,
                            )
                            .await?
                        }
                        Some(AddAccountSubcommand::ApiKey) => {
                            return api_key_add_is_unsupported().map(|_| ());
                        }
                    };
                    debug_assert!(
                        !registered.provider_account_id.is_empty(),
                        "registered ChatGPT provider account id should not be empty"
                    );
                    println!(
                        "registered account: {} pool={}",
                        registered.account_id, registered.pool_id
                    );
                    Ok(())
                }
                AccountsSubcommand::ImportLegacy(command) => {
                    let imported = import_legacy_account(
                        &runtime,
                        &config,
                        command.pool.as_deref(),
                        account_pool.as_deref(),
                    )
                    .await?;
                    println!(
                        "imported legacy account: {} pool={}",
                        imported.account_id, imported.pool_id
                    );
                    Ok(())
                }
                AccountsSubcommand::Enable(command) => {
                    set_account_enabled(&runtime, &command.account_id, true).await
                }
                AccountsSubcommand::Disable(command) => {
                    set_account_enabled(&runtime, &command.account_id, false).await
                }
                AccountsSubcommand::Remove(command) => {
                    remove_account(&runtime, &command.account_id).await
                }
                AccountsSubcommand::List => list_accounts(&runtime).await,
                AccountsSubcommand::Pool(command) => match command.subcommand {
                    PoolSubcommand::List => list_account_pools(&runtime).await,
                    PoolSubcommand::Assign(command) => {
                        assign_account_pool(&runtime, &command.account_id, &command.pool_id).await
                    }
                    PoolSubcommand::Default(command) => match command.subcommand {
                        PoolDefaultSubcommand::Set(command) => {
                            set_default_pool(
                                &runtime,
                                &config,
                                account_pool.as_deref(),
                                &command.pool_id,
                            )
                            .await
                        }
                        PoolDefaultSubcommand::Clear => {
                            clear_default_pool(&runtime, &config, account_pool.as_deref()).await
                        }
                    },
                    PoolSubcommand::Show(_) => {
                        unreachable!("handled before runtime initialization")
                    }
                },
                AccountsSubcommand::Diagnostics(_) => {
                    unreachable!("handled before runtime initialization")
                }
                AccountsSubcommand::Events(_) => {
                    unreachable!("handled before runtime initialization")
                }
                AccountsSubcommand::Current(current_command) => {
                    let diagnostic =
                        read_current_diagnostic(&runtime, &config, account_pool.as_deref())
                            .await
                            .context("read account startup preview")?;
                    if current_command.json {
                        print_current_json(&diagnostic)?;
                    } else {
                        print_current_text(&diagnostic);
                    }
                    Ok(())
                }
                AccountsSubcommand::Status(status_command) => {
                    let diagnostic =
                        read_status_diagnostic(&runtime, &config, account_pool.as_deref())
                            .await
                            .context("read account startup status")?;
                    if status_command.json {
                        print_status_json(&diagnostic)?;
                    } else {
                        print_status_text(&diagnostic);
                    }
                    Ok(())
                }
                AccountsSubcommand::Resume => {
                    let selection = runtime
                        .read_account_startup_selection()
                        .await
                        .context("read account startup selection")?;
                    runtime
                        .write_account_startup_selection(AccountStartupSelectionUpdate {
                            default_pool_id: selection.default_pool_id,
                            preferred_account_id: None,
                            suppressed: false,
                        })
                        .await
                        .context("clear account startup selection suppression")?;
                    println!("automatic selection resumed");
                    Ok(())
                }
                AccountsSubcommand::Switch(command) => {
                    let current =
                        read_current_diagnostic(&runtime, &config, account_pool.as_deref())
                            .await
                            .context("read account startup preview")?;
                    let Some(effective_pool_id) =
                        current.startup.startup.preview.effective_pool_id.clone()
                    else {
                        anyhow::bail!(
                            "no effective pool is configured for pooled account selection"
                        );
                    };
                    let membership = runtime
                        .read_account_pool_membership(&command.account_id)
                        .await
                        .context("read preferred account membership")?
                        .ok_or_else(|| {
                            anyhow::anyhow!("account `{}` is not registered", command.account_id)
                        })?;
                    if membership.pool_id != effective_pool_id {
                        anyhow::bail!(
                            "account `{}` belongs to pool `{}`; current effective pool is `{effective_pool_id}`",
                            command.account_id,
                            membership.pool_id
                        );
                    }
                    let selection = runtime
                        .read_account_startup_selection()
                        .await
                        .context("read account startup selection")?;
                    runtime
                        .write_account_startup_selection(AccountStartupSelectionUpdate {
                            default_pool_id: selection.default_pool_id,
                            preferred_account_id: Some(command.account_id.clone()),
                            suppressed: false,
                        })
                        .await
                        .context("write preferred account startup selection")?;
                    println!("preferred account: {}", command.account_id);
                    Ok(())
                }
            }
        }
    }
}

fn validate_explicit_observability_target(
    command_pool: Option<&str>,
    top_level_override: Option<&str>,
) -> anyhow::Result<()> {
    if command_pool.is_some() || top_level_override.is_some() {
        let _ = resolve_target_pool(command_pool, top_level_override, None)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AccountsCommand;
    use clap::Parser;

    #[test]
    fn accounts_pool_default_commands_parse() {
        let set = AccountsCommand::try_parse_from(["codex", "pool", "default", "set", "team-main"])
            .expect("default set parses");
        assert!(format!("{set:?}").contains("Default"));

        let clear = AccountsCommand::try_parse_from(["codex", "pool", "default", "clear"])
            .expect("default clear parses");
        assert!(format!("{clear:?}").contains("Clear"));
    }
}
