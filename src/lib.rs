//! Simulation & Benchmark environment for Solana's Proprietary AMMs.
//!
//! Simulate and/or Benchmark swaps across *any* of the major Solana Proprietary AMMs, locally, using LiteSVM.
#![doc = include_str!("../README.md")]
#![allow(clippy::type_complexity, clippy::result_large_err, clippy::too_many_arguments)]

pub mod builder;
pub mod cfg;
pub mod env;
pub mod misc;

use std::{
    collections::HashMap,
    fs::{self, File},
    path::Path,
    thread,
};

use chrono::Local;
use clap::{Args, Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use magnus_router_client::types::SwapArgs;
use magnus_shared::{Dex, Route};
use polars::prelude::{Column, DataFrame, ParquetWriter};
use secrecy::{ExposeSecret, SecretString};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{account::Account, instruction::Instruction, pubkey, pubkey::Pubkey, transaction::Transaction};
use tracing::{debug, info, warn};

use crate::{
    builder::ConstructSwap,
    cfg::{Cfg, PMMTarget, Swap},
    env::Environment,
    misc::Misc,
};

/// Constants used throughout the simulation environment.
/// Holds the CFG file paths, templates, limits and more;
pub mod consts {
    use solana_sdk::{pubkey, pubkey::Pubkey};

    pub const ROUTER: &str = "magnus-router";
    pub const DATASETS_PATH: &str = "./datasets";
    pub const SETUP_PATH: &str = "./cfg/setup.toml";
    pub const ACCOUNTS_PATH: &str = "./cfg/accounts";
    pub const PROGRAMS_PATH: &str = "./cfg/programs";

    pub const PROGRESS_CHARS: &str = "█▓░";
    pub const PROGRESS_TEMPLATE: &str = "{prefix:>12.bold} [{bar:40.cyan/blue}] {pos:>6}/{len:<6} ({percent}%)";

    // used to pay for tx fees
    pub const AIRDROP_AMOUNT: u64 = 100_000_000_000;
    // the maximum number of compute units a tx can consume
    pub const COMPUTE_UNITS_LIMIT: u64 = 20_000_000;

    pub const JITODONTFRONT: Pubkey = pubkey!("jitodontfront11111111111JustUseJupiterU1tra");
}

#[derive(Parser, Debug)]
#[command(version, about = "Simulation environment for Solana's Proprietary AMMs.\nSimulate swaps and Benchmark performance across *any* of the major Solana Prop AMMs.", long_about = None)]
pub struct CliArgs {
    #[command(subcommand)]
    pub cmd: Cmd,
}

impl CliArgs {
    fn parse_nested_pmms(s: &str) -> Result<Vec<Vec<PMMTarget>>, String> {
        if let Ok(parsed) = serde_json::from_str::<Vec<Vec<String>>>(s) {
            return parsed
                .into_iter()
                .map(|grp| grp.into_iter().map(|s| s.parse::<PMMTarget>()).collect::<Result<Vec<PMMTarget>, _>>())
                .collect();
        }

        let s = s.trim();
        if !s.starts_with("[[") || !s.ends_with("]]") {
            return Err("invalid format: expected [[dex1,dex2],[dex3]]".to_string());
        }

        let inner = &s[1..s.len() - 1];
        inner
            .split("],[")
            .map(|grp| {
                let grp = grp.trim_matches('[').trim_matches(']');
                grp.split(',').map(|s| s.trim().parse::<PMMTarget>()).collect::<Result<Vec<PMMTarget>, _>>()
            })
            .collect()
    }

    fn parse_nested_weights(s: &str) -> Result<Vec<Vec<u8>>, String> {
        serde_json::from_str(s).map_err(|e| format!("invalid format: {}", e))
    }

    fn parse_range(s: &str) -> Result<[f64; 3], String> {
        let parts: Vec<f64> = s.split(',').map(|p| p.trim().parse::<f64>().map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?;

        let parts: [f64; 3] =
            parts.try_into().map_err(|v: Vec<f64>| format!("expected exactly 3 values (start,end,step), got {}", v.len()))?;

        if parts[0] >= parts[1] {
            return Err("start must be less than end".to_string());
        }

        if parts[2] <= 0.0 {
            return Err("step must be positive".to_string());
        }

        Ok(parts)
    }

    fn default_pmm() -> Vec<PMMTarget> {
        Dex::PMM.iter().map(|d| PMMTarget { dex: *d, market_hint: None }).collect()
    }
}

#[derive(Args, Debug)]
pub struct CommonArgs {
    #[arg(long, env = "HTTP_URL", default_value = "https://api.mainnet.solana.com")]
    pub http_url: SecretString,

    #[arg(
        long,
        env = "JIT_ACCOUNTS",
        action = clap::ArgAction::Set,
        default_value_t = true,
        help = "Fetch accounts at runtime (use --jit-accounts=false for loading the local ones instead)"
    )]
    pub jit_accounts: bool,

    #[arg(
        long,
        env = "JIT_PROGRAMS",
        action = clap::ArgAction::Set,
        default_value_t = true,
        help = "Fetch programs at runtime (use --jit-programs=false for loading the local ones instead)"
    )]
    pub jit_programs: bool,

    #[arg(long, env = "SRC_TOKEN", default_value = "wsol", help = "Source token symbol")]
    pub src_token: String,

    #[arg(long, env = "DST_TOKEN", default_value = "usdc", help = "Destination token symbol")]
    pub dst_token: String,

    #[arg(long, env = "SETUP_PATH", default_value = consts::SETUP_PATH, help = "Path to the setup configuration file")]
    pub setup_path: String,

    #[arg(long, env = "PROGRAMS_PATH", default_value = consts::PROGRAMS_PATH, help = "Directory to load programs from")]
    pub programs_path: String,

    #[arg(long, env = "ACCOUNTS_PATH", default_value = consts::ACCOUNTS_PATH, help = "Directory to load accounts from")]
    pub accounts_path: String,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    #[command(about = "Initialize an environment for a single PMM and execute a direct swap.")]
    Direct {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(long, help = "The Prop AMM to use (optionally with market hint, e.g. humidifi_Fk)")]
        pmm: PMMTarget,

        #[arg(long, env = "AMOUNT_IN", default_value_t = 1.0, help = "The amount of tokens to trade")]
        amount_in: f64,
    },

    #[command(
        about = "Fetch accounts from the specified Pmms via RPC and save them locally (presumably for later usage).",
        after_help = "Examples:
  pmm-sim fetch-accounts --pmms=humidifi
  pmm-sim fetch-accounts --pmms=humidifi,obric-v2,zerofi,solfi-v2
  pmm-sim \
                      fetch-accounts --pmms=humidifi --http-url=https://my-rpc.com"
    )]
    FetchAccounts {
        #[arg(long, env = "HTTP_URL", default_value = "https://api.mainnet.solana.com")]
        http_url: SecretString,

        #[arg(long, env = "SETUP_PATH", default_value = consts::SETUP_PATH, help = "Path to the setup configuration file")]
        setup_path: String,

        #[arg(long, env = "ACCOUNTS_PATH", default_value = consts::ACCOUNTS_PATH, help = "Directory to save fetched accounts")]
        accounts_path: String,

        #[arg(
            long,
            value_delimiter = ',',
            default_values_t = CliArgs::default_pmm(),
            help = "Comma-separated list of Prop AMMs to fetch accounts for"
        )]
        pmms: Vec<PMMTarget>,
    },

    #[command(
        alias = "single",
        about = "Run a single swap route across one or more Prop AMMs with specified weights.",
        after_help = "Examples:
  pmm-sim single --pmms=humidifi --weights=100 --amount-in=100 --src-token=WSOL --dst-token=USDC
  pmm-sim single --spoof=okxlabs --pmms=humidifi,solfi-v2 --weights=50,50 --amount-in=150000 --src-token=USDC --dst-token=WSOL
  pmm-sim single --amount-in=10000 --pmms=solfi-v2,tessera --weights=30,70
  pmm-sim single --spoof=jupiter --amount-in=10000 --pmms=obric-v2 --weights=100 --src-token=USDC --dst-token=USDT"
    )]
    RouterSingle {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(
            long,
            value_delimiter = ',',
            default_value = "humidifi,solfi-v2",
            help = "Comma-separated list of Prop AMMs (optionally with market hint, e.g. humidifi_Fk)"
        )]
        pmms: Vec<PMMTarget>,

        #[arg(long, env = "SPOOF", help = "Spoof as an aggregator for CPI calls")]
        spoof: Option<Aggregator>,

        #[arg(long, env = "AMOUNT_IN", default_value_t = 1.0, help = "The amount of tokens to trade")]
        amount_in: f64,

        #[arg(long, value_delimiter = ',', default_value = "50,50", help = "Comma-separated weights")]
        weights: Vec<u8>,

        #[arg(long, env = "JITODONTFRONT", action = clap::ArgAction::Set, default_value_t = false, help = "Append jitodontfront account to remaining_accounts")]
        jitodontfront: bool,

        #[arg(long, env = "JITODONTFRONT_ACC", help = "Override the default jitodontfront account address")]
        jitodontfront_acc: Option<Pubkey>,
    },

    #[command(
        alias = "multi",
        about = "Execute multiple swap routes across nested Prop AMM routes. Each inner list represents a single route, each route \
                 possibly going through multiple Prop AMMs.",
        after_help = "Examples:

  # Execute a two-fold multi-route (WSOL -> USDC) swap. The first route goes through humidifi and goonfi, the second route goes through \
                      solfi-v2. The swap amounts are 311 WSOL and 234 WSOL respectively.
  pmm-sim multi --pmms='[[humidifi,goonfi],[solfi-v2]]' --weights='[[50,50],[100]]' --src-token=wsol --dst-token=usdc --amount-in=311,234

  # Execute a three-fold multi-route (WSOL -> USDC) swap. The first route goes through humidifi and tessera, the second route goes through \
                      solfi-v2, and the third route goes through goonfi and humidifi. The swap amounts are 10 WSOL, 50 WSOL, and 350 WSOL \
                      respectively.
  pmm-sim multi --amount-in=10,50,350 --pmms='[[humidifi,tessera],[solfi-v2],[goonfi,humidifi]]' --weights='[[11,89],[100],[5,95]]'

  # Execute a three-fold multi-route (USDC -> WSOL) swap: The first route goes through humidifi and solfi-v2, the second route goes \
                      through goonfi, and the third route goes through solfi-v2 and goonfi. The swap amounts are 15K USDC, 1K USDC, and \
                      33K USDC respectively.
  pmm-sim multi --pmms='[[humidifi,solfi-v2],[goonfi],[solfi-v2,goonfi]]' --weights='[[25,75],[100],[33,67]]' \
                      --amount-in=150000,1000,33000 --src-token=USDC --dst-token=WSOL --jit-accounts=true"
    )]
    RouterMulti {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(long, default_value = "[[humidifi,solfi-v2],[solfi-v2]]", help = "JSON nested routes, e.g. '[[dex1,dex2],[dex3]]'")]
        pmms: String,

        #[arg(long, env = "SPOOF", help = "Spoof as an aggregator for CPI calls")]
        spoof: Option<Aggregator>,

        #[arg(
            long,
            env = "AMOUNT_IN",
            value_delimiter = ',',
            num_args = 1..,
            default_values_t = vec![1.0, 1.0],
            help = "Comma-separated amounts for each route, e.g. --amount-in=3,50"
        )]
        amount_in: Vec<f64>,

        #[arg(
            long,
            default_value = "[[30,70],[100]]",
            help = "JSON nested weights matching the prop AMMs structure, e.g. '[[50,50],[100]]'"
        )]
        weights: String,

        #[arg(long, env = "JITODONTFRONT", action = clap::ArgAction::Set, default_value_t = false, help = "Append jitodontfront account to remaining_accounts")]
        jitodontfront: bool,

        #[arg(long, env = "JITODONTFRONT_ACC", help = "Override the default jitodontfront account address")]
        jitodontfront_acc: Option<Pubkey>,
    },

    #[command(
        about = "Benchmark swaps for any one of the implemented Prop AMMs by specifying, optionally, the accounts, src/dst tokens and \
                 step size",
        after_help = "Examples:
  # Benchmark swaps on HumidiFi,Tessera,SolfiV2 and GoonFi, from 1 to 4000 WSOL to USDC, in increments of 1 WSOL.
  pmm-sim benchmark --range=1.0,4000.0,1.0 --pmms=humidifi,tessera,solfi-v2,goonfi --src-token=wsol --dst-token=usdc

  Benchmark swaps on Tessera and SolfiV2, from 1 to 250 WSOL, in increments of 0.01 WSOL.
  pmm-sim benchmark --range=1.0,250.0,0.01 --pmms=tessera,solfi-v2 --src-token=wsol --dst-token=usdc
        "
    )]
    Benchmark {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(
            long,
            env = "PROP_AMMS",
            value_delimiter = ',',
            default_value = "humidifi",
            help = "The Prop AMMs to benchmark (optionally with market hint, e.g. humidifi_Fk)"
        )]
        pmms: Vec<PMMTarget>,

        #[arg(long, env = "SPOOF", help = "Spoof as an aggregator for CPI calls")]
        spoof: Option<Aggregator>,

        #[arg(long, env = "DATASETS_PATH", default_value = consts::DATASETS_PATH, help = "Directory to dump the benchmark parquet files into")]
        datasets_path: String,

        #[arg(long, env = "RANGE", default_value = "1.0,100.0,1.0", value_parser = CliArgs::parse_range, help = "Comma-separated step parameters: start, end, step")]
        range: [f64; 3],

        #[arg(
            long,
            env = "CALL_TYPE",
            default_value = "cpi",
            help = "Swap call type: cpi (through router) or direct (standalone PMM instruction)"
        )]
        call_type: CallType,

        #[arg(long, env = "JITODONTFRONT", action = clap::ArgAction::Set, default_value_t = false, help = "Append jitodontfront account to remaining_accounts")]
        jitodontfront: bool,

        #[arg(long, env = "JITODONTFRONT_ACC", help = "Override the default jitodontfront account address")]
        jitodontfront_acc: Option<Pubkey>,
    },

    #[command(
        about = "Fetch programs from the specified Pmms via RPC and save them locally (presumably for later usage).",
        after_help = "Examples:
  pmm-sim fetch-programs --pmms=humidifi
  pmm-sim fetch-programs --pmms=humidifi,obric-v2,zerofi,solfi-v2
  pmm-sim \
                      fetch-programs --pmms=humidifi --http-url=https://my-rpc.com"
    )]
    FetchPrograms {
        #[arg(long, env = "HTTP_URL", default_value = "https://api.mainnet.solana.com")]
        http_url: SecretString,

        #[arg(long, env = "SETUP_PATH", default_value = consts::SETUP_PATH, help = "Path to the setup configuration file")]
        setup_path: String,

        #[arg(long, env = "PROGRAMS_PATH", default_value = consts::PROGRAMS_PATH, help = "Directory to save fetched programs")]
        programs_path: String,

        #[arg(
            long,
            value_delimiter = ',',
            default_values_t = CliArgs::default_pmm(),
            help = "Comma-separated list of Prop AMMs to fetch programs for"
        )]
        pmms: Vec<PMMTarget>,
    },
}

impl Cmd {
    pub fn setup_path(&self) -> &str {
        match self {
            Cmd::FetchAccounts { setup_path, .. } | Cmd::FetchPrograms { setup_path, .. } => setup_path,
            Cmd::Benchmark { common, .. }
            | Cmd::RouterSingle { common, .. }
            | Cmd::RouterMulti { common, .. }
            | Cmd::Direct { common, .. } => &common.setup_path,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Cmd::FetchAccounts { .. } => "FetchAccounts",
            Cmd::FetchPrograms { .. } => "FetchPrograms",
            Cmd::Benchmark { .. } => "Benchmark",
            Cmd::RouterSingle { .. } => "SingleRouteSwaps",
            Cmd::RouterMulti { .. } => "MultiRouteSwaps",
            Cmd::Direct { .. } => "Direct",
        }
    }
}

/// The Aggregators we can spoof as, when doing CPI calls
#[derive(Debug, Copy, Clone, clap::ValueEnum)]
pub enum Aggregator {
    #[value(name = "dflow")]
    DFlow,
    Jupiter,
    #[value(name = "okxlabs")]
    OkxLabs,
    Titan,
}

impl Aggregator {
    pub fn program_id(&self) -> Pubkey {
        match self {
            Aggregator::Jupiter => pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"),
            Aggregator::DFlow => pubkey!("DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH"),
            Aggregator::OkxLabs => pubkey!("6m2CDdhRgxpH4WjvdzxAYbGxwdGUz5MziiL5jek2kBma"),
            Aggregator::Titan => pubkey!("T1TANpTeScyeqVzzgNViGDNrkQ6qHz9KrSBS4aNXvGT"),
        }
    }
}

impl std::fmt::Display for Aggregator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Aggregator::DFlow => write!(f, "dflow"),
            Aggregator::Jupiter => write!(f, "jupiter"),
            Aggregator::OkxLabs => write!(f, "okxlabs"),
            Aggregator::Titan => write!(f, "titan"),
        }
    }
}

#[derive(clap::ValueEnum, Clone, Debug, Default)]
pub enum CallType {
    #[default]
    Cpi,
    Direct,
}

#[derive(Debug, Clone)]
pub struct BenchmarkRecord {
    slot: u64,
    pmm: String,
    market: String,
    src_token: String,
    dst_token: String,
    amount_in: f64,
    amount_out: f64,
    spread: f64,
    spread_bps: f64,
    compute_units: u64,
}

#[derive(Debug, Default, Clone)]
pub struct Benchmark {
    records: Vec<BenchmarkRecord>,
    save_path: String,
    range: [u64; 3],
    time: String,
}

impl Benchmark {
    pub fn new() -> Self {
        Self { time: Local::now().format("%Y%m%d-%H%M%S").to_string(), ..Default::default() }
    }

    pub fn records(mut self, records: Vec<BenchmarkRecord>) -> Self {
        self.records = records;
        self
    }

    pub fn save_path(mut self, path: &str) -> Self {
        self.save_path = if path.ends_with(".parquet") { path.to_string() } else { format!("{}.parquet", path) };
        self
    }

    /// Sets the range from raw values [start, end, step].
    pub fn range(mut self, range: [u64; 3]) -> Self {
        self.range = range;
        self
    }

    /// Sets the range from human-readable values, converting using token decimals.
    pub fn range_from_human(mut self, range: [f64; 3], dec: u8) -> Self {
        self.range = [Misc::to_raw(range[0], dec), Misc::to_raw(range[1], dec), Misc::to_raw(range[2], dec)];
        self
    }

    /// Returns the total number of iterations for this range configuration.
    pub fn range_count(&self) -> u64 {
        (self.range[1] - self.range[0]) / self.range[2] + 1
    }

    /// Returns an iterator over all values from start to end (inclusive).
    pub fn range_iter(&self) -> impl Iterator<Item = u64> {
        let [start, end, step] = self.range;
        (start..=end).step_by(step as usize)
    }

    /// Returns the end value of the range.
    pub fn range_end(&self) -> u64 {
        self.range[1]
    }

    /// Returns the time string we append to the filename.
    pub fn time(&self) -> &str {
        &self.time
    }

    pub fn save(&self) -> eyre::Result<()> {
        let df = DataFrame::new(vec![
            Column::new("slot".into(), self.records.iter().map(|r| r.slot).collect::<Vec<_>>()),
            Column::new("pmm".into(), self.records.iter().map(|r| r.pmm.as_str()).collect::<Vec<_>>()),
            Column::new("market".into(), self.records.iter().map(|r| r.market.as_str()).collect::<Vec<_>>()),
            Column::new("src_token".into(), self.records.iter().map(|r| r.src_token.as_str()).collect::<Vec<_>>()),
            Column::new("dst_token".into(), self.records.iter().map(|r| r.dst_token.as_str()).collect::<Vec<_>>()),
            Column::new("amount_in".into(), self.records.iter().map(|r| r.amount_in).collect::<Vec<_>>()),
            Column::new("amount_out".into(), self.records.iter().map(|r| r.amount_out).collect::<Vec<_>>()),
            Column::new("spread".into(), self.records.iter().map(|r| r.spread).collect::<Vec<_>>()),
            Column::new("spread_bps".into(), self.records.iter().map(|r| r.spread_bps).collect::<Vec<_>>()),
            Column::new("compute_units".into(), self.records.iter().map(|r| r.compute_units).collect::<Vec<_>>()),
        ])?;

        let mut file = File::create(&self.save_path)?;
        ParquetWriter::new(&mut file).finish(&mut df.clone())?;

        Ok(())
    }
}

pub struct App {
    args: CliArgs,
    cfg: Cfg,
}

impl App {
    pub fn new(args: CliArgs, cfg: Cfg) -> Self {
        Self { args, cfg }
    }

    pub fn start(&self) -> eyre::Result<()> {
        match &self.args.cmd {
            Cmd::FetchAccounts { .. } => self.fetch_accounts(),
            Cmd::FetchPrograms { .. } => self.fetch_programs(),
            Cmd::Benchmark { .. } => self.benchmark(),
            Cmd::RouterSingle { .. } | Cmd::RouterMulti { .. } => self.simulate(),
            Cmd::Direct { .. } => self.direct(),
        }
    }

    pub fn fetch_accounts(&self) -> eyre::Result<()> {
        let Cmd::FetchAccounts { http_url, accounts_path, pmms, .. } = &self.args.cmd else { unreachable!() };

        let rpc_client = RpcClient::new(http_url.expose_secret().to_string());
        let dexes: Vec<Dex> = pmms.iter().map(|t| t.dex).collect();
        let (slot, fetched) = Misc::fetch_accounts(&dexes, &rpc_client, &self.cfg)?;

        fetched.iter().try_for_each(|(dex, markets)| -> eyre::Result<()> {
            markets.iter().try_for_each(|(_, accs)| -> eyre::Result<()> {
                accs.iter().try_for_each(|(pubkey, acc)| -> eyre::Result<()> {
                    Misc::save_account_to_disk(accounts_path, dex, pubkey, acc, slot)?;
                    info!("saved account {pubkey} for {dex}");
                    Ok(())
                })
            })
        })?;

        info!("done fetching accounts at slot {slot}");
        Ok(())
    }

    pub fn fetch_programs(&self) -> eyre::Result<()> {
        let Cmd::FetchPrograms { http_url, programs_path, pmms, .. } = &self.args.cmd else { unreachable!() };

        let rpc_client = RpcClient::new(http_url.expose_secret().to_string());
        let dexes: Vec<Dex> = pmms.iter().map(|t| t.dex).collect();
        let programs = Misc::fetch_programs(&dexes, &rpc_client)?;

        let data_dir = Path::new(programs_path.as_str());
        if !data_dir.exists() {
            fs::create_dir_all(data_dir)?;
        }

        programs.iter().try_for_each(|(dex, _program_id, elf_bytes)| -> eyre::Result<()> {
            let path = data_dir.join(format!("{}.so", dex));
            fs::write(&path, elf_bytes)?;
            info!("saved program {dex} to {}", path.display());
            Ok(())
        })?;

        info!("done fetching {} program(s)", programs.len());
        Ok(())
    }

    pub fn benchmark(&self) -> eyre::Result<()> {
        let Cmd::Benchmark { common, datasets_path, pmms, range, spoof, call_type, jitodontfront, jitodontfront_acc } = &self.args.cmd
        else {
            unreachable!()
        };

        let jitodontfront_pubkey = jitodontfront_acc.unwrap_or(consts::JITODONTFRONT);
        let rpc_client = RpcClient::new(common.http_url.expose_secret().to_string());

        let (src_token, dst_token) = (self.cfg.get_token(&common.src_token)?, self.cfg.get_token(&common.dst_token)?);
        let mints = vec![(src_token.addr, src_token.dec), (dst_token.addr, dst_token.dec)];

        let benchmark = Benchmark::new().range_from_human(*range, src_token.dec);
        let multi = MultiProgress::new();

        // Resolve each target to (Dex, market Pubkey)
        let resolved: Vec<(Dex, Pubkey)> =
            pmms.iter().map(|t| (t.dex, t.resolve(&self.cfg).unwrap_or_else(|| panic!("{} not configured", t)))).collect();
        let dexes: Vec<Dex> = resolved.iter().map(|(d, _)| *d).collect();

        // Flatten per-market accounts into per-dex for set_accounts compatibility
        let (slot, accs_map): (Option<u64>, HashMap<Dex, Vec<(Pubkey, Account)>>) = if common.jit_accounts {
            let (s, m) = Misc::fetch_accounts(&dexes, &rpc_client, &self.cfg)?;
            let flat: HashMap<Dex, Vec<(Pubkey, Account)>> =
                m.into_iter().map(|(dex, markets)| (dex, markets.into_iter().flat_map(|(_, accs)| accs).collect())).collect();
            (Some(s), flat)
        } else {
            Misc::read_accounts_from_disk(&dexes, &common.accounts_path)?
        };

        thread::scope(|s| {
            let handles: Vec<_> = resolved
                .iter()
                .zip(pmms.iter())
                .map(|(&(pmm, market), target)| {
                    let (app, cfg, multi, mints) = (&self, &self.cfg, &multi, &mints);
                    let rpc_client = &rpc_client;
                    let pmm_accs = accs_map.get(&pmm).cloned().unwrap_or_default();
                    let benchmark = benchmark.clone();
                    let market_str = market.to_string();

                    s.spawn(move || -> eyre::Result<()> {
                        let spoof_for_programs = if matches!(call_type, CallType::Direct) { None } else { *spoof };

                        // start up the progress bar only when all the spawned threads
                        // have finished bootstrapping so there's no CLI progress bar race cond
                        let (mut env, src_ata, dst_ata) =
                            multi.suspend(|| -> eyre::Result<_> {
                                let mut env = Environment::new(&common.programs_path, &common.accounts_path, Some(mints), slot)?;
                                env.get_and_load_programs(&[pmm], common.jit_programs, spoof_for_programs, Some(rpc_client))?
                                    .setup_wallet(&src_token.addr, benchmark.range_end(), consts::AIRDROP_AMOUNT)?;

                                let (src_ata, dst_ata) = (env.wallet_ata(&src_token.addr), env.wallet_ata(&dst_token.addr));
                                Ok((env, src_ata, dst_ata))
                            })?;

                        let pb = multi.add(ProgressBar::new(benchmark.range_count()));
                        pb.set_style(
                            ProgressStyle::default_bar().template(consts::PROGRESS_TEMPLATE)?.progress_chars(consts::PROGRESS_CHARS),
                        );
                        pb.set_prefix(format!("{}", target));

                        let (mut records, mut warn_cnt) = (vec![], u64::default());
                        let routes: Vec<Vec<magnus_router_client::types::Route>> =
                            vec![vec![Route { dexes: vec![pmm], weights: vec![100] }.into()]];

                        benchmark.range_iter().try_for_each(|amount_in| -> eyre::Result<()> {
                            env.reset_wallet(&src_token.addr, amount_in)?;
                            env.set_accounts(&pmm_accs)?;

                            let ix = match call_type {
                                CallType::Cpi => {
                                    let data = SwapArgs {
                                        amount_in,
                                        expect_amount_out: 1,
                                        min_return: 1,
                                        amounts: vec![amount_in],
                                        routes: routes.clone(),
                                    };

                                    let mut construct = ConstructSwap {
                                        cfg: cfg.clone(),
                                        remaining_accounts: vec![],
                                        payer: env.wallet_pubkey(),
                                        src_ta: src_ata,
                                        dst_ta: dst_ata,
                                        src_mint: src_token.addr,
                                        dst_mint: dst_token.addr,
                                    };
                                    construct.attach_pmms_accs(&[(pmm, market)]);
                                    if *jitodontfront {
                                        construct.append_acc(jitodontfront_pubkey);
                                    }
                                    construct.instruction(*spoof, data, Misc::gen_order_id())
                                }
                                CallType::Direct => {
                                    app.build_direct_ix(&env, target, market, src_token, dst_token, src_ata, dst_ata, amount_in)
                                }
                            };

                            let tx = Transaction::new_signed_with_payer(
                                &[ix],
                                Some(&env.wallet_pubkey()),
                                &[&env.wallet],
                                env.latest_blockhash(),
                            );

                            let res = match env.send_transaction(tx) {
                                Ok(res) => res,
                                Err(e) => {
                                    (warn_cnt == 0).then(|| pb.println(format!("[WARN] {}: {:?}", pmm, e)));
                                    warn_cnt += 1;
                                    pb.inc(1);
                                    return Ok(());
                                }
                            };

                            let amount_out = match call_type {
                                CallType::Cpi => env.get_amount_out(&res),
                                CallType::Direct => env.token_balance(&dst_token.addr),
                            };

                            // reverse swap to measure spread
                            env.set_accounts(&pmm_accs)?;
                            env.reset_wallet(&dst_token.addr, amount_out)?;

                            let rev_ix = match call_type {
                                CallType::Cpi => {
                                    let data = SwapArgs {
                                        amount_in: amount_out,
                                        expect_amount_out: 1,
                                        min_return: 1,
                                        amounts: vec![amount_out],
                                        routes: routes.clone(),
                                    };
                                    let mut construct = ConstructSwap {
                                        cfg: cfg.clone(),
                                        remaining_accounts: vec![],
                                        payer: env.wallet_pubkey(),
                                        src_ta: dst_ata,
                                        dst_ta: src_ata,
                                        src_mint: dst_token.addr,
                                        dst_mint: src_token.addr,
                                    };
                                    construct.attach_pmms_accs(&[(pmm, market)]);
                                    if *jitodontfront {
                                        construct.append_acc(jitodontfront_pubkey);
                                    }
                                    construct.instruction(*spoof, data, Misc::gen_order_id())
                                }
                                CallType::Direct => {
                                    app.build_direct_ix(&env, target, market, dst_token, src_token, dst_ata, src_ata, amount_out)
                                }
                            };

                            let rev_tx = Transaction::new_signed_with_payer(
                                &[rev_ix],
                                Some(&env.wallet_pubkey()),
                                &[&env.wallet],
                                env.latest_blockhash(),
                            );

                            let (spread, spread_bps) = match env.send_transaction(rev_tx) {
                                Ok(rev_res) => {
                                    let amount_back = match call_type {
                                        CallType::Cpi => env.get_amount_out(&rev_res),
                                        CallType::Direct => env.token_balance(&src_token.addr),
                                    };
                                    let amount_in_h = Misc::to_human(amount_in, src_token.dec);
                                    let amount_back_h = Misc::to_human(amount_back, src_token.dec);
                                    let spread = amount_in_h - amount_back_h;
                                    let spread_bps = if amount_in_h > 0.0 { spread / amount_in_h * 10_000.0 } else { 0.0 };
                                    (spread, spread_bps)
                                }
                                Err(e) => {
                                    (warn_cnt == 0).then(|| pb.println(format!("[WARN] {}: reverse swap failed: {:?}", pmm, e)));
                                    warn_cnt += 1;
                                    (f64::NAN, f64::NAN)
                                }
                            };

                            records.push(BenchmarkRecord {
                                slot: env.slot.unwrap_or_default(),
                                pmm: pmm.to_string(),
                                market: market_str.clone(),
                                src_token: src_token.symbol.clone(),
                                dst_token: dst_token.symbol.clone(),
                                amount_in: Misc::to_human(amount_in, src_token.dec),
                                amount_out: Misc::to_human(amount_out, dst_token.dec),
                                spread,
                                spread_bps,
                                compute_units: res.compute_units_consumed,
                            });

                            pb.set_message(format!("in: {:.2}", Misc::to_human(amount_in, src_token.dec)));
                            pb.inc(1);

                            Ok(())
                        })?;

                        (warn_cnt != 0).then(|| pb.println(format!("[WARN] {}: {} total failures", pmm, warn_cnt)));

                        let via = match call_type {
                            CallType::Cpi => spoof.map(|a| a.to_string()).unwrap_or_else(|| "magnus".to_string()),
                            CallType::Direct => "direct".to_string(),
                        };
                        let filename = format!(
                            "{}/{}_{}_{}_{}_{}",
                            datasets_path,
                            env.slot.unwrap_or_default(),
                            via,
                            pmm,
                            market_str,
                            benchmark.time()
                        );
                        benchmark.records(records.clone()).save_path(&filename).save().is_ok().then(|| {
                            pb.println(format!("[{}] saved {} records to {}", pmm, records.len(), filename));
                        });

                        Ok(())
                    })
                })
                .collect();

            handles.into_iter().for_each(move |handle| {
                if let Err(e) = handle.join().expect("thread panicked") {
                    warn!(?e, "benchmark thread failed");
                }
            });
        });

        Ok(())
    }

    pub fn simulate(&self) -> eyre::Result<()> {
        let (common, amount_in, resolved, weights, spoof, jitodontfront, jitodontfront_acc) = match &self.args.cmd {
            Cmd::RouterSingle { common, amount_in, pmms, weights, spoof, jitodontfront, jitodontfront_acc } => {
                let resolved: Vec<Vec<(Dex, Pubkey)>> =
                    vec![pmms.iter().map(|t| (t.dex, t.resolve(&self.cfg).unwrap_or_else(|| panic!("{} not configured", t)))).collect()];
                (common, vec![*amount_in], resolved, vec![weights.clone()], spoof, jitodontfront, jitodontfront_acc)
            }
            Cmd::RouterMulti { common, amount_in, pmms, weights, spoof, jitodontfront, jitodontfront_acc } => {
                let targets = CliArgs::parse_nested_pmms(pmms).expect("invalid format for nested dexes");
                let weights = CliArgs::parse_nested_weights(weights).expect("invalid format for nested weights");

                let resolved: Vec<Vec<(Dex, Pubkey)>> = targets
                    .iter()
                    .map(|grp| {
                        grp.iter().map(|t| (t.dex, t.resolve(&self.cfg).unwrap_or_else(|| panic!("{} not configured", t)))).collect()
                    })
                    .collect();

                (common, amount_in.clone(), resolved, weights, spoof, jitodontfront, jitodontfront_acc)
            }
            _ => unreachable!(),
        };

        // ensure that each dex group has a corresponding weight group
        assert_eq!(resolved.len(), weights.len(), "dexes and weights outer length mismatch");
        resolved.iter().zip(weights.iter()).for_each(|(d, w)| {
            assert_eq!(d.len(), w.len(), "dexes and weights length mismatch");
        });

        let jitodontfront_pubkey = jitodontfront_acc.unwrap_or(consts::JITODONTFRONT);
        let rpc_client = RpcClient::new(common.http_url.expose_secret().to_string());
        let flat_resolved: Vec<(Dex, Pubkey)> = resolved.iter().flatten().copied().collect();
        let flat_dexes: Vec<Dex> = flat_resolved.iter().map(|(d, _)| *d).collect();

        let (src_token, dst_token) = (self.cfg.get_token(&common.src_token)?, self.cfg.get_token(&common.dst_token)?);
        let mints = vec![(src_token.addr, src_token.dec), (dst_token.addr, dst_token.dec)];

        let amount_in: Vec<u64> = amount_in.iter().map(|amount| Misc::to_raw(*amount, src_token.dec)).collect();
        let amount_in_sum: u64 = amount_in.iter().sum();

        let mut env = Environment::new(&common.programs_path, &common.accounts_path, Some(&mints), None)?;
        env.get_and_load_programs(&flat_dexes, common.jit_programs, *spoof, Some(&rpc_client))?
            .get_and_load_accounts(&flat_dexes, common.jit_accounts, Some(&rpc_client), Some(&self.cfg))?
            .setup_wallet(&src_token.addr, amount_in_sum, consts::AIRDROP_AMOUNT)?;
        info!(?env);

        let (src_ata, src_before) = (env.wallet_ata(&src_token.addr), env.token_balance_norm(&src_token.addr, src_token.dec));
        let (dst_ata, dst_before) = (env.wallet_ata(&dst_token.addr), env.token_balance_norm(&dst_token.addr, dst_token.dec));

        debug!(?src_token.symbol, ?src_before, ?dst_token.symbol, ?dst_before);

        let routes: Vec<Vec<magnus_router_client::types::Route>> = resolved
            .iter()
            .zip(weights.iter())
            .map(|(grp, weight_grp)| {
                let dexes: Vec<Dex> = grp.iter().map(|(d, _)| *d).collect();
                vec![Route { dexes, weights: weight_grp.clone() }.into()]
            })
            .collect();

        let data = SwapArgs { amount_in: amount_in_sum, expect_amount_out: 1, min_return: 1, amounts: amount_in, routes: routes.clone() };

        let mut construct = ConstructSwap {
            cfg: self.cfg.clone(),
            remaining_accounts: vec![],
            payer: env.wallet_pubkey(),
            src_ta: src_ata,
            dst_ta: dst_ata,
            src_mint: src_token.addr,
            dst_mint: dst_token.addr,
        };
        construct.attach_pmms_accs(&flat_resolved);
        if *jitodontfront {
            construct.append_acc(jitodontfront_pubkey);
        }
        let swap_ix = construct.instruction(*spoof, data, Misc::gen_order_id());

        let tx = Transaction::new_signed_with_payer(&[swap_ix], Some(&env.wallet_pubkey()), &[&env.wallet], env.latest_blockhash());
        let res = env.send_transaction(tx).expect("failed to exec tx");
        let amount_out = env.get_amount_out(&res);

        env.get_router_swap_events(&res).iter().for_each(|event| {
            info!(?event);
        });
        info!(
            src_token = %src_token.symbol,
            dst_token = %dst_token.symbol,
            routes = ?routes,
            amount_in = ?Misc::to_human(amount_in_sum, src_token.dec),
            amount_out = ?Misc::to_human(amount_out, dst_token.dec),
            cu = res.compute_units_consumed
        );

        Ok(())
    }

    fn build_direct_ix<P: Into<String> + std::fmt::Display + Clone + std::fmt::Debug>(
        &self,
        env: &Environment<P>,
        pmm: &PMMTarget,
        market: Pubkey,
        src_token: &cfg::Token,
        dst_token: &cfg::Token,
        src_ata: Pubkey,
        dst_ata: Pubkey,
        amount_in: u64,
    ) -> Instruction {
        match pmm.dex {
            Dex::SolfiV2 => {
                let cfg = self
                    .cfg
                    .solfi_v2
                    .as_ref()
                    .and_then(|c| c.swap_v1.get(&market))
                    .unwrap_or_else(|| panic!("SolFiV2 market {market} not configured"));

                let (direction, user_base_ta, user_quote_ta) = if src_token.addr == cfg.base_mint && dst_token.addr == cfg.quote_mint {
                    (0u8, src_ata, dst_ata)
                } else if src_token.addr == cfg.quote_mint && dst_token.addr == cfg.base_mint {
                    (1u8, dst_ata, src_ata)
                } else {
                    panic!("src/dst token mints don't match solfi-v2 market base/quote mints");
                };

                Instruction {
                    program_id: Pubkey::new_from_array(magnus_shared::pmm_solfi_v2::id().to_bytes()),
                    accounts: cfg.swap_accounts(env.wallet_pubkey(), user_base_ta, user_quote_ta, None, None),
                    data: cfg.instruction_data(&[], amount_in, direction),
                }
            }
            Dex::Tessera => {
                let cfg = self
                    .cfg
                    .tessera
                    .as_ref()
                    .and_then(|c| c.swap_v1.get(&market))
                    .unwrap_or_else(|| panic!("Tessera market {market} not configured"));

                let base_mint = env.token_account_mint(&cfg.base_ta);
                let quote_mint = env.token_account_mint(&cfg.quote_ta);

                // side: 1 = base→quote (sell), 0 = quote→base (buy)
                let (side, user_base_ta, user_quote_ta) = if src_token.addr == base_mint && dst_token.addr == quote_mint {
                    (1u8, src_ata, dst_ata)
                } else if src_token.addr == quote_mint && dst_token.addr == base_mint {
                    (0u8, dst_ata, src_ata)
                } else {
                    panic!("src/dst token mints don't match tessera market base/quote mints");
                };

                Instruction {
                    program_id: Pubkey::new_from_array(magnus_shared::pmm_tessera::id().to_bytes()),
                    accounts: cfg.swap_accounts(env.wallet_pubkey(), user_base_ta, user_quote_ta, Some(base_mint), Some(quote_mint)),
                    data: cfg.instruction_data(&[], amount_in, side),
                }
            }
            Dex::BisonFi => {
                let cfg = self
                    .cfg
                    .bisonfi
                    .as_ref()
                    .and_then(|c| c.swap_v1.get(&market))
                    .unwrap_or_else(|| panic!("BisonFi market {market} not configured"));

                let base_mint = env.token_account_mint(&cfg.market_base_ta);

                let (b_to_a, user_base_ta, user_quote_ta) = if src_token.addr == base_mint {
                    (false, src_ata, dst_ata)
                } else {
                    (true, dst_ata, src_ata)
                };

                Instruction {
                    program_id: Pubkey::new_from_array(magnus_shared::pmm_bisonfi::id().to_bytes()),
                    accounts: cfg.swap_accounts(env.wallet_pubkey(), user_base_ta, user_quote_ta, None, None),
                    data: cfg.instruction_data(&[], amount_in, b_to_a as u8),
                }
            }
            Dex::HumidiFi | Dex::HumidiFiSwapV2 | Dex::HumidiFiSwapV3 => {
                let humidifi = self.cfg.humidifi.as_ref().unwrap_or_else(|| panic!("HumidiFi not configured"));

                let (accounts, data) = match pmm.dex {
                    Dex::HumidiFi => {
                        let c = humidifi.swap_v1.get(&market).unwrap_or_else(|| panic!("HumidiFi v1 market {market} not configured"));
                        let base_mint = env.token_account_mint(&c.base_ta);
                        let (ba, qa) = if src_token.addr == base_mint { (src_ata, dst_ata) } else { (dst_ata, src_ata) };
                        let is_quote_to_base: u8 = if src_token.addr == base_mint { 0 } else { 1 };
                        (
                            c.swap_accounts(env.wallet_pubkey(), ba, qa, None, None),
                            c.instruction_data(magnus_shared::pmm_humidifi::SWAP_SELECTOR, amount_in, is_quote_to_base),
                        )
                    }
                    Dex::HumidiFiSwapV2 => {
                        let c = humidifi.swap_v2.get(&market).unwrap_or_else(|| panic!("HumidiFi v2 market {market} not configured"));
                        let base_mint = env.token_account_mint(&c.base_ta);
                        let (ba, qa) = if src_token.addr == base_mint { (src_ata, dst_ata) } else { (dst_ata, src_ata) };
                        let is_quote_to_base: u8 = if src_token.addr == base_mint { 0 } else { 1 };
                        (
                            c.swap_accounts(env.wallet_pubkey(), ba, qa, None, None),
                            c.instruction_data(magnus_shared::pmm_humidifi::SWAPV2_SELECTOR, amount_in, is_quote_to_base),
                        )
                    }
                    Dex::HumidiFiSwapV3 => {
                        let c = humidifi.swap_v3.get(&market).unwrap_or_else(|| panic!("HumidiFi v3 market {market} not configured"));
                        let base_mint = env.token_account_mint(&c.base_ta);
                        let (ba, qa) = if src_token.addr == base_mint { (src_ata, dst_ata) } else { (dst_ata, src_ata) };
                        let is_quote_to_base: u8 = if src_token.addr == base_mint { 0 } else { 1 };
                        (
                            c.swap_accounts(env.wallet_pubkey(), ba, qa, None, None),
                            c.instruction_data(magnus_shared::pmm_humidifi::SWAPV3_SELECTOR, amount_in, is_quote_to_base),
                        )
                    }
                    _ => unreachable!(),
                };

                Instruction { program_id: Pubkey::new_from_array(magnus_shared::pmm_humidifi::id().to_bytes()), accounts, data }
            }
            Dex::GoonFi => {
                let cfg = self
                    .cfg
                    .goonfi
                    .as_ref()
                    .and_then(|c| c.swap_v1.get(&market))
                    .unwrap_or_else(|| panic!("GoonFi market {market} not configured"));

                let quote_mint = env.token_account_mint(&cfg.quote_ta);

                // is_bid: true when buying base with quote
                let (is_bid, base_account, quote_account) = if src_token.addr == quote_mint {
                    (true, dst_ata, src_ata)
                } else {
                    (false, src_ata, dst_ata)
                };

                Instruction {
                    program_id: Pubkey::new_from_array(magnus_shared::pmm_goonfi::id().to_bytes()),
                    accounts: cfg.swap_accounts(env.wallet_pubkey(), base_account, quote_account, None, None),
                    data: cfg.instruction_data(&[], amount_in, is_bid as u8),
                }
            }
            Dex::ObricV2 => {
                let cfg = self
                    .cfg
                    .obric_v2
                    .as_ref()
                    .and_then(|c| c.swap_v2.get(&market))
                    .unwrap_or_else(|| panic!("ObricV2 market {market} not configured"));

                let x_mint = env.token_account_mint(&cfg.reserve_x);
                let y_mint = env.token_account_mint(&cfg.reserve_y);

                let (x_to_y, user_token_x, user_token_y) = if src_token.addr == x_mint && dst_token.addr == y_mint {
                    (true, src_ata, dst_ata)
                } else if src_token.addr == y_mint && dst_token.addr == x_mint {
                    (false, dst_ata, src_ata)
                } else {
                    panic!("src/dst token mints don't match obric-v2 market x/y mints");
                };

                Instruction {
                    program_id: Pubkey::new_from_array(magnus_shared::pmm_obric_v2::id().to_bytes()),
                    accounts: cfg.swap_accounts(env.wallet_pubkey(), user_token_x, user_token_y, None, None),
                    data: cfg.instruction_data(&[], amount_in, x_to_y as u8),
                }
            }
            _ => unimplemented!("direct swap not yet implemented for {:?}", pmm.dex),
        }
    }

    pub fn direct(&self) -> eyre::Result<()> {
        let Cmd::Direct { common, pmm, amount_in } = &self.args.cmd else { unreachable!() };

        let market = pmm.resolve(&self.cfg).unwrap_or_else(|| panic!("{} not configured", pmm));
        let rpc_client = RpcClient::new(common.http_url.expose_secret().to_string());

        let (src_token, dst_token) = (self.cfg.get_token(&common.src_token)?, self.cfg.get_token(&common.dst_token)?);
        let mints = vec![(src_token.addr, src_token.dec), (dst_token.addr, dst_token.dec)];
        let amount_in = Misc::to_raw(*amount_in, src_token.dec);

        let mut env = Environment::new(&common.programs_path, &common.accounts_path, Some(&mints), None)?;
        env.get_and_load_programs(&[pmm.dex], common.jit_programs, None, Some(&rpc_client))?
            .get_and_load_accounts(&[pmm.dex], common.jit_accounts, Some(&rpc_client), Some(&self.cfg))?
            .setup_wallet(&src_token.addr, amount_in, consts::AIRDROP_AMOUNT)?;
        info!(?env);

        let (src_ata, dst_ata) = (env.wallet_ata(&src_token.addr), env.wallet_ata(&dst_token.addr));
        let (src_before, dst_before) = (env.token_balance(&src_token.addr), env.token_balance(&dst_token.addr));

        let ix = self.build_direct_ix(&env, pmm, market, src_token, dst_token, src_ata, dst_ata, amount_in);
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&env.wallet_pubkey()), &[&env.wallet], env.latest_blockhash());
        let res = env.send_transaction(tx).expect("failed to exec tx");

        let (src_after, dst_after) = (env.token_balance(&src_token.addr), env.token_balance(&dst_token.addr));

        info!(
            pmm = %pmm,
            market = %market,
            src_token = %src_token.symbol,
            dst_token = %dst_token.symbol,
            amount_in = ?Misc::to_human(amount_in, src_token.dec),
            src_balance = ?format!("{:.6} -> {:.6}", Misc::to_human(src_before, src_token.dec), Misc::to_human(src_after, src_token.dec)),
            dst_balance = ?format!("{:.6} -> {:.6}", Misc::to_human(dst_before, dst_token.dec), Misc::to_human(dst_after, dst_token.dec)),
            cu = res.compute_units_consumed,
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod cli {
        use super::*;

        fn t(dex: Dex) -> PMMTarget {
            PMMTarget { dex, market_hint: None }
        }

        #[test]
        fn test_parse_nested_pmms_json_single() {
            let input = r#"[["humidifi"]]"#;
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![t(Dex::HumidiFi)]]);
        }

        #[test]
        fn test_parse_nested_pmms_json_multiple() {
            let input = r#"[["humidifi","obric-v2"],["zerofi"]]"#;
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![t(Dex::HumidiFi), t(Dex::ObricV2)], vec![t(Dex::ZeroFi)]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_single() {
            let input = "[[humidifi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![t(Dex::HumidiFi)]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_single_route_multiple_pmms() {
            let input = "[[humidifi,obric-v2]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![t(Dex::HumidiFi), t(Dex::ObricV2)]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_multiple_routes() {
            let input = "[[humidifi,obric-v2],[zerofi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![t(Dex::HumidiFi), t(Dex::ObricV2)], vec![t(Dex::ZeroFi)]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_three_routes() {
            let input = "[[humidifi],[obric-v2,solfi-v2],[zerofi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![t(Dex::HumidiFi)], vec![t(Dex::ObricV2), t(Dex::SolfiV2)], vec![t(Dex::ZeroFi)]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_all_pmms() {
            let input = "[[raydium-cl-v2,raydium-cp],[obric-v2,solfi-v2,zerofi,humidifi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(
                result,
                vec![
                    vec![t(Dex::RaydiumClV2), t(Dex::RaydiumCp)],
                    vec![t(Dex::ObricV2), t(Dex::SolfiV2), t(Dex::ZeroFi), t(Dex::HumidiFi)]
                ]
            );
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_with_spaces() {
            let input = "[[ humidifi , obric-v2 ],[ zerofi ]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![t(Dex::HumidiFi), t(Dex::ObricV2)], vec![t(Dex::ZeroFi)]]);
        }

        fn th(dex: Dex, hint: &str) -> PMMTarget {
            PMMTarget { dex, market_hint: Some(hint.to_string()) }
        }

        #[test]
        fn test_parse_nested_pmms_with_market_hints() {
            let input = "[[humidifi_Fk,goonfi_4u],[solfi-v2_65]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![th(Dex::HumidiFi, "Fk"), th(Dex::GoonFi, "4u")], vec![th(Dex::SolfiV2, "65")]]);
        }

        #[test]
        fn test_parse_nested_pmms_mixed_hints_and_plain() {
            let input = "[[humidifi_Fk,obric-v2],[zerofi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![th(Dex::HumidiFi, "Fk"), t(Dex::ObricV2)], vec![t(Dex::ZeroFi)]]);
        }

        #[test]
        fn test_parse_nested_pmms_json_with_market_hints() {
            let input = r#"[["humidifi_Fk","tessera_FL"],["solfi-v2"]]"#;
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![th(Dex::HumidiFi, "Fk"), th(Dex::Tessera, "FL")], vec![t(Dex::SolfiV2)]]);
        }

        #[test]
        fn test_parse_nested_pmms_invalid_pmm() {
            let input = "[[humidifi,invalid-dex]]";
            let result = CliArgs::parse_nested_pmms(input);
            assert!(result.is_err());
        }

        #[test]
        fn test_parse_nested_pmms_invalid_format() {
            let input = "[humidifi]"; // not nested
            let result = CliArgs::parse_nested_pmms(input);
            assert!(result.is_err());
        }

        #[test]
        fn test_parse_nested_weights_single() {
            let input = "[[100]]";
            let result = CliArgs::parse_nested_weights(input).unwrap();
            assert_eq!(result, vec![vec![100u8]]);
        }

        #[test]
        fn test_parse_nested_weights_multiple() {
            let input = "[[50,50],[100]]";
            let result = CliArgs::parse_nested_weights(input).unwrap();
            assert_eq!(result, vec![vec![50u8, 50u8], vec![100u8]]);
        }

        #[test]
        fn test_parse_nested_weights_complex() {
            let input = "[[30,30,40],[60,40],[100]]";
            let result = CliArgs::parse_nested_weights(input).unwrap();
            assert_eq!(result, vec![vec![30u8, 30u8, 40u8], vec![60u8, 40u8], vec![100u8]]);
        }

        #[test]
        fn test_pmms_and_weights_match() {
            let pmms_input = "[[humidifi,obric-v2],[zerofi]]";
            let weights_input = "[[50,50],[100]]";

            let pmms = CliArgs::parse_nested_pmms(pmms_input).unwrap();
            let weights = CliArgs::parse_nested_weights(weights_input).unwrap();

            assert_eq!(pmms.len(), weights.len());
            pmms.iter().zip(weights.iter()).for_each(|(d, w)| {
                assert_eq!(d.len(), w.len());
            });
        }

        #[test]
        fn test_parse_range_valid() {
            let result = CliArgs::parse_range("1.0,100.0,0.5").unwrap();
            assert_eq!(result, [1.0, 100.0, 0.5]);
        }

        #[test]
        fn test_parse_range_with_spaces() {
            let result = CliArgs::parse_range("1.0, 100.0, 0.5").unwrap();
            assert_eq!(result, [1.0, 100.0, 0.5]);
        }

        #[test]
        fn test_parse_range_start_gte_end() {
            let result = CliArgs::parse_range("100.0,50.0,1.0");
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("start must be less than end"));
        }

        #[test]
        fn test_parse_range_negative_step() {
            let result = CliArgs::parse_range("1.0,100.0,-1.0");
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("step must be positive"));
        }
    }
}
