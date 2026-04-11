use anyhow::Context;
use clap::Args;
use clap::Parser;
use codex_core::config::Config;
use codex_state::AccountStartupEligibility;
use codex_state::AccountStartupSelectionPreview;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::StateRuntime;
use codex_state::state_db_path;
use codex_utils_cli::CliConfigOverrides;

#[derive(Debug, Parser)]
pub struct AccountsCommand {
    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    #[command(subcommand)]
    pub subcommand: AccountsSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum AccountsSubcommand {
    Add(AddAccountCommand),
    List,
    Current,
    Status,
    Resume,
    Switch(SwitchAccountCommand),
}

#[derive(Debug, Args)]
pub struct AddAccountCommand {
    #[arg(value_name = "ACCOUNT_ID")]
    pub account_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct SwitchAccountCommand {
    #[arg(value_name = "ACCOUNT_ID")]
    pub account_id: String,
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
    let cli_overrides = command
        .config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    let config = Config::load_with_cli_overrides(cli_overrides).await?;
    let runtime = StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
        .await
        .context("initialize account startup selection state")?;
    let selection = runtime
        .read_account_startup_selection()
        .await
        .context("read account startup selection")?;
    let startup_preview = runtime
        .preview_account_startup_selection(configured_default_pool_id(&config))
        .await
        .context("preview account startup selection")?;

    match command.subcommand {
        AccountsSubcommand::Add(_command) => {
            anyhow::bail!("`codex accounts add` is not implemented yet")
        }
        AccountsSubcommand::List => {
            let Some(accounts) = config.accounts.as_ref() else {
                println!("No account pools configured.");
                return Ok(());
            };
            let Some(pools) = accounts.pools.as_ref() else {
                println!("No account pools configured.");
                return Ok(());
            };
            if pools.is_empty() {
                println!("No account pools configured.");
                return Ok(());
            }

            for pool_id in pools.keys() {
                println!("{pool_id}");
            }
            Ok(())
        }
        AccountsSubcommand::Current => {
            print_preview(&startup_preview);
            println!(
                "automatic selection: {}",
                if startup_preview.suppressed {
                    "suppressed"
                } else {
                    "enabled"
                }
            );
            Ok(())
        }
        AccountsSubcommand::Status => {
            let pool_count = config
                .accounts
                .as_ref()
                .and_then(|accounts| accounts.pools.as_ref())
                .map_or(0, |pools| pools.len());
            println!(
                "suppression: {}",
                if startup_preview.suppressed {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            print_preview(&startup_preview);
            println!("configured pools: {pool_count}");
            Ok(())
        }
        AccountsSubcommand::Resume => {
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
            let Some(effective_pool_id) = startup_preview.effective_pool_id.clone() else {
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
            runtime
                .write_account_startup_selection(AccountStartupSelectionUpdate {
                    default_pool_id: Some(effective_pool_id),
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

fn configured_default_pool_id(config: &Config) -> Option<&str> {
    config
        .accounts
        .as_ref()
        .and_then(|accounts| accounts.default_pool.as_deref())
}

fn print_preview(preview: &AccountStartupSelectionPreview) {
    println!(
        "effective pool: {}",
        preview.effective_pool_id.as_deref().unwrap_or("none")
    );
    println!(
        "preferred account: {}",
        preview
            .preferred_account_id
            .as_deref()
            .unwrap_or("automatic")
    );
    println!(
        "predicted account: {}",
        preview.predicted_account_id.as_deref().unwrap_or("none")
    );
    println!("eligibility: {}", format_eligibility(&preview.eligibility));
}

fn format_eligibility(eligibility: &AccountStartupEligibility) -> String {
    match eligibility {
        AccountStartupEligibility::Suppressed => {
            "automatic pooled selection is suppressed".to_string()
        }
        AccountStartupEligibility::MissingPool => "no effective pool is configured".to_string(),
        AccountStartupEligibility::PreferredAccountSelected => {
            "preferred account is eligible for fresh-runtime startup".to_string()
        }
        AccountStartupEligibility::AutomaticAccountSelected => {
            "automatic startup selection is eligible".to_string()
        }
        AccountStartupEligibility::PreferredAccountMissing => {
            "preferred account is not registered".to_string()
        }
        AccountStartupEligibility::PreferredAccountInOtherPool { actual_pool_id } => {
            format!("preferred account belongs to pool `{actual_pool_id}`")
        }
        AccountStartupEligibility::PreferredAccountUnhealthy => {
            "preferred account is unhealthy".to_string()
        }
        AccountStartupEligibility::PreferredAccountBusy => {
            "preferred account is currently leased by another runtime".to_string()
        }
        AccountStartupEligibility::NoEligibleAccount => {
            "no eligible account is available in the effective pool".to_string()
        }
    }
}
