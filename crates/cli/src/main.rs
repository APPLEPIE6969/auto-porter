//! `auto-porter` binary (the [[bin]] must NOT be called "cli", spec §1).

use clap::{Parser, Subcommand, ValueEnum};
use shared::config::PorterConfig;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TierChoice {
    Auto,
    A,
    B,
    C,
    D,
}

#[derive(Parser)]
#[command(
    name = "auto-porter",
    version,
    about = "Java -> Horizon mod porter: .jar in, .hmod/.hnmod + PortReport out"
)]
struct Cli {
    /// Path to porter.toml (defaults to ./porter.toml; missing file -> defaults)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Path to native-hooks.json (overrides porter.toml hooks_json; falls back
    /// to the in-tree seed catalog)
    #[arg(long, global = true)]
    hooks_json: Option<PathBuf>,
    /// LLM provider override: anthropic | openai | openrouter | nvidia
    /// (overrides porter.toml [llm].provider)
    #[arg(long, global = true)]
    llm_provider: Option<String>,
    /// LLM model id override (overrides porter.toml [llm].model)
    #[arg(long, global = true)]
    llm_model: Option<String>,
    /// Verbose logging on stderr
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Port a single mod jar into .hmod/.hnmod + PortReport
    Port {
        /// The Java mod jar (Fabric, NeoForge or legacy Forge; 1.21+ up to 26.3)
        jar: PathBuf,
        /// Tier override: auto follows the classifier
        #[arg(long, value_enum, default_value = "auto")]
        tier: TierChoice,
        /// Full 40-hex Horizon commit to pin (overrides porter.toml)
        #[arg(long)]
        horizon_revision: Option<String>,
        /// Stop after the data phase (used by CI)
        #[arg(long)]
        data_only: bool,
        #[arg(long, default_value = "./output/")]
        output: PathBuf,
    },
    /// Port a modpack: a directory of jars, a Modrinth .mrpack, or a CurseForge zip
    PortPack {
        path: PathBuf,
        #[arg(long)]
        horizon_revision: Option<String>,
        #[arg(long, default_value = "./output/")]
        output: PathBuf,
        #[arg(long)]
        continue_on_error: bool,
    },
    /// Verify an output directory: --static / --compile / --parity / --in-game
    Verify {
        /// Output dir containing <mod-id>.hmod / .hnmod / -report.json
        path: PathBuf,
        #[arg(long)]
        r#static: bool,
        #[arg(long)]
        compile: bool,
        #[arg(long)]
        parity: bool,
        #[arg(long)]
        in_game: bool,
    },
    /// Decompose + classify only; writes decomposed.json and classified.json
    Analyze {
        jar: PathBuf,
        #[arg(long, default_value = "./out/")]
        output: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    let mut cfg = match &cli.config {
        Some(p) => PorterConfig::load(p),
        None => {
            let p = PathBuf::from("porter.toml");
            PorterConfig::load(&p)
        }
    };
    // Global overrides beat porter.toml — CI and users switch providers
    // without rewriting the config file.
    if let Some(p) = &cli.llm_provider {
        cfg.llm.provider = p.clone();
    }
    if let Some(m) = &cli.llm_model {
        cfg.llm.model = m.clone();
    }

    let exit = run(cli, cfg);
    std::process::exit(exit);
}

fn run(cli: Cli, cfg: PorterConfig) -> i32 {
    match cli.command {
        Command::Port {
            jar,
            tier,
            horizon_revision,
            data_only,
            output,
        } => {
            let _ = tier; // auto is the only mode; explicit tiers land with the
                          // per-feature override surface in a later sprint.
            let opts = auto_porter_lib::PortOptions {
                horizon_revision,
                data_only,
                output,
                verbose: cli.verbose,
                hooks_json: cli.hooks_json,
            };
            match auto_porter_lib::port(&jar, &cfg, &opts) {
                Ok(out) => {
                    print_summary(&out.report);
                    if out.report.overall == shared::report::Overall::Failed {
                        1
                    } else {
                        0
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        Command::PortPack {
            path,
            horizon_revision,
            output,
            continue_on_error,
        } => {
            let opts = auto_porter_lib::PortOptions {
                horizon_revision,
                data_only: false,
                output,
                verbose: cli.verbose,
                hooks_json: cli.hooks_json,
            };
            match auto_porter_lib::port_pack(&path, &cfg, &opts, continue_on_error) {
                Ok(summary) => {
                    println!(
                        "port-pack: {} ported, {} failures",
                        summary.get("ported").and_then(|v| v.as_u64()).unwrap_or(0),
                        summary
                            .get("failures")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        Command::Verify {
            path,
            r#static,
            compile,
            parity,
            in_game,
        } => match auto_porter_lib::verify_dir(
            &path,
            &cfg,
            r#static,
            compile,
            parity,
            in_game,
            cli.verbose,
        ) {
            Ok(v) => {
                println!(
                    "{}: static schema={} hooks={} deps={} parity={} in_game={} overall={}",
                    v.mod_id,
                    v.r#static.schema.as_str(),
                    v.r#static.hooks.as_str(),
                    v.r#static.deps.as_str(),
                    v.parity.status.as_str(),
                    v.in_game.status.as_str(),
                    v.overall.as_str()
                );
                for issue in &v.r#static.issues {
                    println!("  [{:?}] {}: {}", issue.severity, issue.code, issue.message);
                }
                for note in &v.human_review_needed {
                    println!("  review: {note}");
                }
                if v.overall == shared::report::Overall::Failed {
                    1
                } else {
                    0
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        Command::Analyze { jar, output } => {
            match auto_porter_lib::analyze(&jar, &output, cli.hooks_json.as_deref()) {
                Ok(classified) => {
                    println!(
                        "analyze: {} feature(s), tiers {:?}; wrote decomposed.json + classified.json to {}",
                        classified.plan.features.len(),
                        classified.plan.tier_summary,
                        output.display()
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
    }
}

fn print_summary(report: &shared::report::PortReport) {
    println!(
        "ported {} ({}): overall {} — {} feature(s): {} ported, {} partial, {} skipped, {} blocked",
        report.mod_id,
        report.loader,
        report.overall.as_str(),
        report.summary.total_features,
        report.summary.ported,
        report.summary.partial,
        report.summary.skipped,
        report.summary.blocked
    );
    println!(
        "outputs: hmod={} hnmod={} carry={}",
        report.outputs.hmod.as_deref().unwrap_or("-"),
        report.outputs.hnmod.as_deref().unwrap_or("-"),
        report.data_carry.as_deref().unwrap_or("-")
    );
    for e in &report.errors {
        println!(
            "  [{}] {}{}: {}",
            e.error_type.as_str(),
            e.phase.as_str(),
            e.feature
                .as_ref()
                .map(|f| format!(" / {f}"))
                .unwrap_or_default(),
            e.message
        );
    }
}
