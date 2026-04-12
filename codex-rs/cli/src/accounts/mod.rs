mod diagnostics;
mod mutate;
mod output;

use anyhow::Context;
use clap::Args;
use clap::Parser;
use codex_core::config::Config;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::StateRuntime;
use codex_state::state_db_path;
use codex_utils_cli::CliConfigOverrides;
use diagnostics::read_current_diagnostic;
use diagnostics::read_status_diagnostic;
use mutate::assign_account_pool;
use mutate::list_account_pools;
use mutate::list_accounts;
use mutate::remove_account;
use mutate::set_account_enabled;
use output::print_current_json;
use output::print_current_text;
use output::print_status_json;
use output::print_status_text;

const ACCOUNTS_ADD_CREDENTIAL_STORAGE_GAP: &str = "pooled credential storage keyed by `credential_ref` is not implemented yet, so `codex accounts add` cannot persist a new pooled account without mutating the shared legacy compatibility auth store. Use `codex login` only if you need to replace the single legacy default compatibility account.";

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
    Enable(AccountToggleCommand),
    Disable(AccountToggleCommand),
    Remove(RemoveAccountCommand),
    List,
    Pool(PoolCommand),
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
}

#[derive(Debug, Args)]
pub struct PoolAssignCommand {
    #[arg(value_name = "ACCOUNT_ID")]
    pub account_id: String,

    #[arg(value_name = "POOL_ID")]
    pub pool_id: String,
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
    let cli_overrides = config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    let config = Config::load_with_cli_overrides(cli_overrides).await?;
    let runtime = StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
        .await
        .context("initialize account startup selection state")?;

    match subcommand {
        AccountsSubcommand::Add(_command) => anyhow::bail!(ACCOUNTS_ADD_CREDENTIAL_STORAGE_GAP),
        AccountsSubcommand::Enable(command) => {
            set_account_enabled(&runtime, &command.account_id, true).await
        }
        AccountsSubcommand::Disable(command) => {
            set_account_enabled(&runtime, &command.account_id, false).await
        }
        AccountsSubcommand::Remove(command) => remove_account(&runtime, &command.account_id).await,
        AccountsSubcommand::List => list_accounts(&runtime).await,
        AccountsSubcommand::Pool(command) => match command.subcommand {
            PoolSubcommand::List => list_account_pools(&runtime).await,
            PoolSubcommand::Assign(command) => {
                assign_account_pool(&runtime, &command.account_id, &command.pool_id).await
            }
        },
        AccountsSubcommand::Current(current_command) => {
            let diagnostic = read_current_diagnostic(&runtime, &config, account_pool.as_deref())
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
            let diagnostic = read_status_diagnostic(&runtime, &config, account_pool.as_deref())
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
            let current = read_current_diagnostic(&runtime, &config, account_pool.as_deref())
                .await
                .context("read account startup preview")?;
            let Some(effective_pool_id) = current.preview.effective_pool_id.clone() else {
                anyhow::bail!("no effective pool is configured for pooled account selection");
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
