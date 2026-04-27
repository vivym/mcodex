use clap::Parser;
use clap::Subcommand;
use codex_smoke_fixtures::SmokeScenario;
use codex_smoke_fixtures::seed_fixture;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "mcodex-smoke-fixture")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Seed(SeedArgs),
}

#[derive(Debug, Parser)]
struct SeedArgs {
    #[arg(long)]
    home: PathBuf,
    #[arg(long, value_enum)]
    scenario: SmokeScenario,
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Seed(args) => {
            let summary = seed_fixture(&args.home, args.scenario).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                println!("seeded scenario={} home={}", summary.scenario, summary.home);
            }
            Ok(())
        }
    }
}
