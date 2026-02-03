//! Simulation & Benchmark environment for Solana's Proprietary AMMs.
//!
//! Simulate and/or Benchmark swaps across *any* of the major Solana Proprietary AMMs, locally, using LiteSVM.
#![doc = include_str!("../README.md")]
#![allow(clippy::type_complexity, clippy::result_large_err)]

use std::{
    collections::{HashMap, HashSet},
    fmt::{Debug, Display},
    fs::{self, File},
    io::Write,
    path::Path,
    str::FromStr,
    thread,
    time::SystemTime,
};

use base64::{Engine, engine::general_purpose};
use chrono::Local;
use clap::{Args, Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use litesvm::{LiteSVM, types::TransactionMetadata};
use magnus_router_client::instructions::SwapBuilder;
use magnus_shared::{Dex, Route};
use polars::prelude::*;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_sdk::{
    account::Account, message::AccountMeta, program_pack::Pack, pubkey::Pubkey, rent::Rent, signature::Keypair, signer::Signer, sysvar,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use tracing::{debug, info, warn};

/// Constants used throughout the simulation environment.
/// Holds the CFG file paths, swappable token accounts and more;
pub mod consts {
    use solana_sdk::{pubkey, pubkey::Pubkey};

    pub const ROUTER: &str = "magnus-router";
    pub const SETUP_PATH: &str = "./setup.toml";
    pub const DATASETS_PATH: &str = "./datasets";
    pub const PROGRAMS_PATH: &str = "./cfg/programs";
    pub const ACCOUNTS_PATH: &str = "./cfg/accounts";

    pub const WSOL: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
    pub const WSOL_DECIMALS: u8 = 9;

    pub const USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    pub const USDC_DECIMALS: u8 = 6;

    pub const USDT: Pubkey = pubkey!("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB");
    pub const USDT_DECIMALS: u8 = 6;

    pub const PROGRESS_CHARS: &str = "█▓░";
    pub const PROGRESS_TEMPLATE: &str = "{prefix:>12.bold} [{bar:40.cyan/blue}] {pos:>6}/{len:<6} ({percent}%)";

    // used to pay for tx fees
    pub const AIRDROP_AMOUNT: u64 = 100_000_000_000;
    // the maximum number of compute units a tx can consume
    pub const COMPUTE_UNITS_LIMIT: u64 = 20_000_000;
}

/// Macro to generate dex configuration structs and their associated functions.
/// All PMM markets have extremely similar, yet distinct, cfgs.
///
/// Necessary to avoid tedious duplicating.
macro_rules! define_dex_configs {
    (
        $(
            $dex_variant:ident => $struct_name:ident : $cfg_field:ident ($toml_key:literal) {
                $( $field:ident ),* $(,)?
            }
        ),* $(,)?
    ) => {
        $(
            #[derive(Debug, Deserialize, Clone)]
            pub struct $struct_name {
                $(
                    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
                    pub $field: Pubkey,
                )*
            }

            impl $struct_name {
                pub fn accounts(&self) -> Vec<Pubkey> {
                    vec![ $( self.$field ),* ]
                }
            }
        )*

        #[derive(Clone, Debug, Deserialize, Default)]
        pub struct PMMCfg {
            $(
                #[serde(rename = $toml_key, default)]
                pub $cfg_field: Option<$struct_name>,
            )*
        }

        impl PMMCfg {
            pub fn load(path: &str) -> eyre::Result<Self> {
                let contents = fs::read_to_string(path)?;
                let cfg: PMMCfg = toml::from_str(&contents)?;
                Ok(cfg)
            }

            pub fn get_accounts(&self, dex: &Dex) -> Option<Vec<Pubkey>> {
                match dex {
                    $(
                        Dex::$dex_variant => self.$cfg_field.as_ref().map(|c| c.accounts()),
                    )*
                    _ => None,
                }
            }

            pub fn get_market(&self, dex: &Dex) -> Option<Pubkey> {
                match dex {
                    $(
                        Dex::$dex_variant => self.$cfg_field.as_ref().map(|c| c.market),
                    )*
                    _ => None,
                }
            }

            pub fn get_config<T: DexCfg>(&self) -> Option<&T> {
                T::from_cfg(self)
            }
        }

        pub trait DexCfg: Sized {
            fn from_cfg(cfg: &PMMCfg) -> Option<&Self>;
        }

        $(
            impl DexCfg for $struct_name {
                fn from_cfg(cfg: &PMMCfg) -> Option<&Self> {
                    cfg.$cfg_field.as_ref()
                }
            }
        )*
    };
}

// All DEX Cfgs
// Reference the cfg file — `setup.toml`
define_dex_configs! {
    HumidiFi => HumidiFiCfg : humidifi ("humidifi") {
        market,
        base_ta,
        quote_ta,
    },
    Tessera => TesseraCfg : tessera ("tessera") {
        market,
        base_ta,
        quote_ta,
        global_state,
    },
    GoonFi => GoonFiCfg : goonfi ("goonfi") {
        market,
        base_ta,
        quote_ta,
        blacklist,
    },
    SolfiV2 => SolfiV2Cfg : solfi_v2 ("solfi-v2") {
        market,
        base_ta,
        quote_ta,
        cfg,
        oracle,
    },
    ZeroFi => ZeroFiCfg : zerofi ("zerofi") {
        market,
        vault_info_base,
        vault_base,
        vault_info_quote,
        vault_quote,
    },
    ObricV2 => ObricV2Cfg : obric_v2 ("obric-v2") {
        market,
        second_ref_oracle,
        third_ref_oracle,
        reserve_x,
        reserve_y,
        ref_oracle,
        x_price_feed,
        y_price_feed,
    },
    BisonFi => BisonfiCfg : bisonfi ("bisonfi") {
        market,
        market_base_ta,
        market_quote_ta,
    },
}

#[derive(Parser, Debug)]
#[command(version, about = "Simulation environment for Solana's Proprietary AMMs.\nSimulate swaps and Benchmark performance across *any* of the major Solana Prop AMMs.", long_about = None)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Command,
}

impl CliArgs {
    fn parse_nested_pmms(s: &str) -> Result<Vec<Vec<Dex>>, String> {
        if let Ok(parsed) = serde_json::from_str::<Vec<Vec<String>>>(s) {
            return parsed.into_iter().map(|group| group.into_iter().map(|s| s.parse::<Dex>()).collect::<Result<Vec<Dex>, _>>()).collect();
        }

        let s = s.trim();
        if !s.starts_with("[[") || !s.ends_with("]]") {
            return Err("invalid format: expected [[dex1,dex2],[dex3]]".to_string());
        }

        let inner = &s[1..s.len() - 1];
        inner
            .split("],[")
            .map(|group| {
                let group = group.trim_matches('[').trim_matches(']');
                group.split(',').map(|dex| dex.trim().parse::<Dex>()).collect::<Result<Vec<Dex>, _>>()
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

    fn default_pmm() -> Vec<Dex> {
        Dex::PMM.to_vec()
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

    #[arg(long, env = "SRC_TOKEN", default_value = "wsol", help = "Source token: wsol, usdc, or usdt")]
    pub src_token: Token,

    #[arg(long, env = "DST_TOKEN", default_value = "usdc", help = "Destination token: wsol, usdc, or usdt")]
    pub dst_token: Token,

    #[arg(long, env = "SETUP_PATH", default_value = consts::SETUP_PATH, help = "Path to the setup configuration file")]
    pub setup_path: String,

    #[arg(long, env = "PROGRAMS_PATH", default_value = consts::PROGRAMS_PATH, help = "Directory to load programs from")]
    pub programs_path: String,

    #[arg(long, env = "ACCOUNTS_PATH", default_value = consts::ACCOUNTS_PATH, help = "Directory to load accounts from")]
    pub accounts_path: String,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(
        about = "Run a single swap route across one or more Prop AMMs with specified weights.",
        after_help = "Examples:
  pmm-sim single --pmms=humidifi --weights=100 --amount-in=100 --src-token=WSOL --dst-token=USDC
  pmm-sim single --pmms=humidifi,solfi-v2 --weights=50,50 --amount-in=150000 --src-token=USDC --dst-token=WSOL
  pmm-sim single --amount-in=10000 --pmms=solfi-v2,tessera --weights=30,70
  pmm-sim single --amount-in=10000 --pmms=obric-v2 --weights=100 --src-token=USDC --dst-token=USDT"
    )]
    Single {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(long, env = "AMOUNT_IN", default_value_t = 1.0, help = "The amount of tokens to trade")]
        amount_in: f64,

        #[arg(long, value_delimiter = ',', default_value = "humidifi,solfi-v2", help = "Comma-separated list of Prop AMMs")]
        pmms: Vec<Dex>,

        #[arg(long, value_delimiter = ',', default_value = "50,50", help = "Comma-separated weights")]
        weights: Vec<u8>,
    },

    #[command(
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
    Multi {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(
            long,
            env = "AMOUNT_IN",
            value_delimiter = ',',
            num_args = 1..,
            default_values_t = vec![1.0, 1.0],
            help = "Comma-separated amounts for each route, e.g. --amount-in=3,50"
        )]
        amount_in: Vec<f64>,

        #[arg(long, default_value = "[[humidifi,solfi-v2],[solfi-v2]]", help = "JSON nested routes, e.g. '[[dex1,dex2],[dex3]]'")]
        pmms: String,

        #[arg(
            long,
            default_value = "[[30,70],[100]]",
            help = "JSON nested weights matching the prop AMMs structure, e.g. '[[50,50],[100]]'"
        )]
        weights: String,
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

        #[arg(long, env = "DATASETS_PATH", default_value = consts::DATASETS_PATH, help = "Directory to dump the benchmark parquet files into")]
        datasets_path: String,

        #[arg(long, env = "PROP_AMMS", value_delimiter = ',', default_value = "humidifi", help = "The Prop AMMs to benchmark")]
        pmms: Vec<Dex>,

        #[arg(long, env = "RANGE", default_value = "1.0,100.0,1.0", value_parser = CliArgs::parse_range, help = "Comma-separated step parameters: start, end, step")]
        range: [f64; 3],
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
        pmms: Vec<Dex>,
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
        pmms: Vec<Dex>,
    },
}

impl Command {
    pub fn setup_path(&self) -> &str {
        match self {
            Command::FetchAccounts { setup_path, .. } | Command::FetchPrograms { setup_path, .. } => setup_path,
            Command::Benchmark { common, .. } => &common.setup_path,
            Command::Single { common, .. } => &common.setup_path,
            Command::Multi { common, .. } => &common.setup_path,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Command::FetchAccounts { .. } => "FetchAccounts",
            Command::FetchPrograms { .. } => "FetchPrograms",
            Command::Benchmark { .. } => "Benchmark",
            Command::Single { .. } => "SingleRouteSwaps",
            Command::Multi { .. } => "MultiRouteSwaps",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    WSOL,
    USDC,
    USDT,
}

impl FromStr for Token {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wsol" | "sol" => Ok(Token::WSOL),
            "usdc" => Ok(Token::USDC),
            "usdt" => Ok(Token::USDT),
            _ => Err(format!("invalid token '{}'. valid options: wsol, usdc, usdt", s)),
        }
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::WSOL => f.write_str("WSOL"),
            Token::USDT => f.write_str("USDT"),
            Token::USDC => f.write_str("USDC"),
        }
    }
}

impl Token {
    fn get_addr(&self) -> Pubkey {
        match *self {
            Token::WSOL => consts::WSOL,
            Token::USDC => consts::USDC,
            Token::USDT => consts::USDT,
        }
    }

    fn get_decimals(&self) -> u8 {
        match *self {
            Token::WSOL => consts::WSOL_DECIMALS,
            Token::USDC => consts::USDC_DECIMALS,
            Token::USDT => consts::USDT_DECIMALS,
        }
    }
}

/// The Simulation Environment;
/// Ensures proper setup of LiteSVM, wallet, programs, and accounts.
/// Also provides utility functions for common operations, like
/// loading programs/accounts, setting up the wallet, sending transactions, etc.
pub struct Environment<'a, P: Into<String> + Display + Clone + Debug> {
    svm: LiteSVM,
    slot: Option<u64>,
    wallet: Keypair,
    mints: Option<&'a [(Pubkey, u8)]>,
    cfg: PMMCfg,

    programs_path: P,
    accounts_path: P,
}

impl<'a, P: Into<String> + Display + Clone + std::fmt::Debug> Debug for Environment<'a, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("slot", &self.slot)
            .field("wallet_pubkey", &self.wallet.pubkey())
            .field("programs_path", &self.programs_path)
            .field("accounts_path", &self.accounts_path)
            .field("mints", &self.mints)
            .finish()
    }
}

impl<'a, P: Into<String> + Display + Clone + Debug> Environment<'a, P> {
    pub fn new(
        programs_path: P,
        accounts_path: P,
        mints: Option<&[(Pubkey, u8)]>,
        cfg: PMMCfg,
        slot: Option<u64>,
    ) -> eyre::Result<Environment<'_, P>> {
        let mut budget = ComputeBudget::new_with_defaults(false);
        budget.compute_unit_limit = consts::COMPUTE_UNITS_LIMIT;

        let wallet = Keypair::new();
        let mut svm = LiteSVM::new().with_default_programs().with_sysvars().with_sigverify(true).with_compute_budget(budget);

        if let Some(mints) = mints {
            mints.iter().try_for_each(|(mint, mint_decimals)| svm.set_account(*mint, Misc::mk_mint_acc(*mint_decimals)))?;
        }

        if let Some(slot) = slot {
            svm.warp_to_slot(slot);
        }

        Ok(Environment { svm, slot, wallet, programs_path, accounts_path, mints, cfg })
    }

    /// Sets up the wallet for the simulation environment.
    ///
    /// This function initializes the wallet's Associated Token Accounts (ATAs) for all
    /// configured mints and funds the wallet with SOL for transaction fees.
    ///
    /// # Arguments
    /// * `src_mint` - The mint address of the source token to fund
    /// * `src_amount` - The amount of source tokens to mint to the wallet's ATA
    /// * `airdrop_amount` - The amount of SOL (in lamports) to airdrop for transaction fees
    ///
    /// # Behavior
    /// 1. Creates ATAs with zero balance for all mints in `self.mints`
    /// 2. Sets the source mint's ATA balance to `src_amount`
    /// 3. Airdrops SOL to the wallet for fees
    pub fn setup_wallet(&mut self, src_mint: &Pubkey, src_amount: u64, airdrop_amount: u64) -> eyre::Result<&mut Self> {
        self.reset_wallet(src_mint, src_amount)?;
        self.svm.airdrop(&self.wallet_pubkey(), airdrop_amount).expect("airdrop failed");

        Ok(self)
    }

    /// Resets the wallet's token balances between simulation iterations.
    ///
    /// This function is used in benchmarking to restore the wallet to a known state
    /// before each swap iteration, ensuring consistent and reproducible results.
    ///
    /// # Arguments
    /// * `src_mint` - The mint address of the source token
    /// * `src_amount` - The amount of source tokens to set in the wallet's ATA
    ///
    /// # Behavior
    /// 1. Sets the source mint's ATA balance to `src_amount`
    /// 2. Resets all other mint ATAs to zero balance
    pub fn reset_wallet(&mut self, src_mint: &Pubkey, src_amount: u64) -> eyre::Result<&mut Self> {
        if let Some(mints) = self.mints {
            mints.iter().try_for_each(|(mint, _)| {
                let ata = self.wallet_ata(mint);
                let amount = if mint == src_mint { src_amount } else { 0 };
                self.svm.set_account(ata, Misc::mk_ata(mint, &self.wallet_pubkey(), amount))
            })?;
        }

        Ok(self)
    }

    pub fn wallet_pubkey(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    pub fn wallet_ata(&self, mint: &Pubkey) -> Pubkey {
        get_associated_token_address(&self.wallet.pubkey(), mint)
    }

    /// The router program is always loaded, plus any unique PMM programs from the provided list.
    pub fn get_and_load_programs(&mut self, pmms: &[Dex], jit: bool, client: Option<&RpcClient>) -> eyre::Result<&mut Self> {
        let pmms: Vec<Dex> = pmms.iter().copied().collect::<HashSet<_>>().into_iter().collect(); // rm duplicates
        self.load_program_router()?; // mandatory load

        match jit {
            true => {
                let rpc_client = client.expect("RPC client is required for JIT program loading");
                self.jit_programs(&pmms, rpc_client)?;
            }
            false => {
                self.static_programs(&pmms)?;
            }
        }

        Ok(self)
    }

    /// Loads the Prop AMM programs by fetching them from RPC.
    pub fn jit_programs(&mut self, pmms: &[Dex], client: &RpcClient) -> eyre::Result<&mut Self> {
        let programs = Misc::fetch_programs(pmms, client)?;

        programs.iter().try_for_each(|(_, program_id, elf_bytes)| self.svm.add_program(*program_id, elf_bytes))?;

        info!("jit-loaded {} program(s)", programs.len());
        Ok(self)
    }

    /// Loads the router program and all required PMM programs from disk.
    pub fn static_programs(&mut self, pmms: &[Dex]) -> eyre::Result<&mut Self> {
        let programs = Misc::read_programs_from_disk(pmms, &self.programs_path.to_string())?;

        programs.into_iter().try_for_each(|(program_id, path)| self.svm.add_program_from_file(program_id, path))?;

        info!("loaded {pmms:?} program(s)");
        Ok(self)
    }

    /// Loads the router program into the SVM.
    pub fn load_program_router(&mut self) -> eyre::Result<&mut Self> {
        self.svm
            .add_program_from_file(magnus_router_client::programs::ROUTER_ID, format!("{}/{}.so", self.programs_path, consts::ROUTER))?;

        Ok(self)
    }

    /// Sets multiple accounts in the SVM state.
    pub fn set_accounts(&mut self, accs: &[(Pubkey, Account)]) -> eyre::Result<&mut Self> {
        accs.iter().try_for_each(|(pubkey, acc)| self.svm.set_account(*pubkey, acc.clone()))?;

        Ok(self)
    }

    /// Fetches and loads PMM accounts either from RPC (JIT) or from disk cache.
    pub fn get_and_load_accounts(&mut self, pmms: &[Dex], jit: bool, client: Option<&RpcClient>) -> eyre::Result<&mut Self> {
        match jit {
            true => {
                let rpc_client = client.expect("RPC client is required for JIT account loading");
                self.jit_accounts(pmms, rpc_client)?;
            }
            false => {
                self.static_accounts(pmms)?;
            }
        }

        Ok(self)
    }

    /// Fetches PMM accounts from RPC and warps to the fetched slot.
    pub fn jit_accounts(&mut self, pmms: &[Dex], client: &RpcClient) -> eyre::Result<&mut Self> {
        let (slot, accs_map) = Misc::fetch_accounts(pmms, client, &self.cfg)?;

        accs_map.iter().try_for_each(|(_, accs)| self.set_accounts(accs).map(|_| ()))?;

        self.svm.warp_to_slot(slot);
        self.slot = Some(slot);

        Ok(self)
    }

    /// Loads PMM accounts from disk cache and warps to the cached slot.
    pub fn static_accounts(&mut self, pmms: &[Dex]) -> eyre::Result<&mut Self> {
        let (slot, accs_map) = Misc::read_accounts_from_disk(pmms, &self.accounts_path.to_string())?;

        accs_map.iter().try_for_each(|(_, accs)| self.set_accounts(accs).map(|_| ()))?;

        if let Some(s) = slot {
            self.svm.warp_to_slot(s);
            self.slot = Some(s);
        }

        Ok(self)
    }

    /// Returns the token balance for the wallet's ATA of the given mint.
    pub fn token_balance(&self, mint: &Pubkey) -> u64 {
        let ata = self.wallet_ata(mint);
        let acc = self.svm.get_account(&ata).unwrap_or_default();
        spl_token::state::Account::unpack(&acc.data).map(|a| a.amount).unwrap_or(0)
    }

    pub fn token_balance_norm(&self, mint: &Pubkey, dec: u8) -> f64 {
        let balance = self.token_balance(mint);

        Misc::to_human(balance, dec)
    }

    pub fn latest_blockhash(&self) -> solana_sdk::hash::Hash {
        self.svm.latest_blockhash()
    }

    pub fn send_transaction(&mut self, tx: Transaction) -> litesvm::types::TransactionResult {
        self.svm.send_transaction(tx)
    }

    /// Extracts the output amount from a swap transaction's logs.
    ///
    /// Parses the `SwapEvent` log emitted by the router program to find the
    /// `amount_out` value. Panics if the event is not found in the logs.
    pub fn get_event_amount_out(&self, metadata: &TransactionMetadata) -> u64 {
        let amount_out: u64 = metadata
            .logs
            .iter()
            .find_map(|log| {
                if log.contains("SwapEvent") {
                    // i.e.: "Program log: SwapEvent { dex: HumidiFi, amount_in: 1000000000, amount_out: 121518066 }"
                    log.split("amount_out: ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
                } else {
                    None
                }
            })
            .expect("couldn't find amount_out in logs");

        amount_out
    }
}

/// A helper struct to construct swap instructions with the required accounts
/// for different Prop AMMs.
///
/// As it currently stands, all swaps pass through the Magnus Router program,
/// which in turn calls the respective Prop AMM program. Therefore, the swap
/// instruction is built using the `SwapBuilder` from the `magnus-router-client`
/// crate, and then the required accounts for the specific Prop AMM are attached.
///
/// The order of the remaining_accounts matters.
pub struct ConstructSwap<'a> {
    cfg: PMMCfg,
    builder: &'a mut SwapBuilder,
    payer: Pubkey,
    src_ta: Pubkey,
    dst_ta: Pubkey,
    src_mint: Pubkey,
    dst_mint: Pubkey,
}

impl<'a> ConstructSwap<'a> {
    fn instruction(&self) -> solana_sdk::instruction::Instruction {
        self.builder.instruction()
    }

    /// Attaches the required remaining accounts for the specified PMM to the swap instruction.
    ///
    /// Each Prop AMM program expects a specific set of accounts in a precise order as
    /// "remaining accounts" on the swap instruction. This method dispatches to the
    /// appropriate PMM-specific attachment function based on the DEX type.
    pub fn attach_pmm_accs(&mut self, pmm: &Dex) -> &mut Self {
        match pmm {
            Dex::HumidiFi => self.attach_humidifi_accs(),
            Dex::SolfiV2 => self.attach_solfiv2_accs(),
            Dex::ZeroFi => self.attach_zerofi_accs(),
            Dex::ObricV2 => self.attach_obric_v2_accs(),
            Dex::Tessera => self.attach_tessera_accs(),
            Dex::GoonFi => self.attach_goonfi_accs(),
            Dex::BisonFi => self.attach_bisonfi_accs(),
            _ => {
                unimplemented!()
            }
        };

        self
    }

    /// Attaches the required remaining accounts for multiple PMMs to the swap instruction.
    pub fn attach_pmms_accs(&mut self, pmms: &[Dex]) -> &mut Self {
        pmms.iter().for_each(|pmm| {
            self.attach_pmm_accs(pmm);
        });

        self
    }

    pub fn attach_solfiv2_accs(&mut self) {
        let Some(cfg) = &self.cfg.solfi_v2 else {
            panic!("SolFiV2 config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&[
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_solfi_v2::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.src_ta, false),
            AccountMeta::new(self.dst_ta, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new_readonly(cfg.oracle, false),
            AccountMeta::new_readonly(cfg.cfg, false),
            AccountMeta::new(cfg.base_ta, false),
            AccountMeta::new(cfg.quote_ta, false),
            AccountMeta::new_readonly(consts::WSOL, false),
            AccountMeta::new_readonly(consts::USDC, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }

    pub fn attach_humidifi_accs(&mut self) {
        let Some(cfg) = &self.cfg.humidifi else {
            panic!("HumidiFi config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&[
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_humidifi::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.src_ta, false),
            AccountMeta::new(self.dst_ta, false),
            AccountMeta::new_readonly(Misc::create_humidifi_param(1500), false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.base_ta, false),
            AccountMeta::new(cfg.quote_ta, false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }

    pub fn attach_zerofi_accs(&mut self) {
        let Some(cfg) = &self.cfg.zerofi else {
            panic!("ZeroFi config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&[
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_zerofi::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.src_ta, false),
            AccountMeta::new(self.dst_ta, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.vault_info_base, false),
            AccountMeta::new(cfg.vault_base, false),
            AccountMeta::new(cfg.vault_info_quote, false),
            AccountMeta::new(cfg.vault_quote, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }

    pub fn attach_obric_v2_accs(&mut self) {
        let Some(cfg) = &self.cfg.obric_v2 else {
            panic!("ObricV2 config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&[
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_obric_v2::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.src_ta, false),
            AccountMeta::new(self.dst_ta, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new_readonly(cfg.second_ref_oracle, false),
            AccountMeta::new_readonly(cfg.third_ref_oracle, false),
            AccountMeta::new(cfg.reserve_x, false),
            AccountMeta::new(cfg.reserve_y, false),
            AccountMeta::new(cfg.ref_oracle, false),
            AccountMeta::new_readonly(cfg.x_price_feed, false),
            AccountMeta::new_readonly(cfg.y_price_feed, false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ]);
    }

    pub fn attach_tessera_accs(&mut self) {
        let Some(cfg) = &self.cfg.tessera else {
            panic!("Tessera config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&[
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_tessera::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.src_ta, false),
            AccountMeta::new(self.dst_ta, false),
            AccountMeta::new_readonly(cfg.global_state, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.base_ta, false),
            AccountMeta::new(cfg.quote_ta, false),
            AccountMeta::new_readonly(self.src_mint, false),
            AccountMeta::new_readonly(self.dst_mint, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }

    pub fn attach_goonfi_accs(&mut self) {
        let Some(cfg) = &self.cfg.goonfi else {
            panic!("GoonFi config is missing, cannot attach accounts.");
        };

        let goonfi_param = Pubkey::new_from_array([0u8; 32]);

        self.builder.add_remaining_accounts(&[
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_goonfi::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.src_ta, false),
            AccountMeta::new(self.dst_ta, false),
            AccountMeta::new_readonly(goonfi_param, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.base_ta, false),
            AccountMeta::new(cfg.quote_ta, false),
            AccountMeta::new_readonly(cfg.blacklist, false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ]);
    }

    pub fn attach_bisonfi_accs(&mut self) {
        let Some(cfg) = &self.cfg.bisonfi else {
            panic!("BisonFi config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&[
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_bisonfi::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.market_base_ta, false),
            AccountMeta::new(cfg.market_quote_ta, false),
            AccountMeta::new(self.src_ta, false),
            AccountMeta::new(self.dst_ta, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }
}

pub struct Misc;
impl Misc {
    pub fn create_humidifi_param(swap_id: u64) -> Pubkey {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&swap_id.to_le_bytes());
        Pubkey::new_from_array(bytes)
    }

    /// Creates fully initialised mint account suitable for use in LiteSVM simulations.
    pub fn mk_mint_acc(decimals: u8) -> Account {
        let mint = spl_token::state::Mint {
            mint_authority: solana_sdk::program_option::COption::None,
            supply: u64::MAX,
            decimals,
            is_initialized: true,
            freeze_authority: Default::default(),
        };

        let mut data = vec![0u8; spl_token::state::Mint::LEN];
        spl_token::state::Mint::pack(mint, &mut data).unwrap();

        Account {
            lamports: Rent::default().minimum_balance(data.len()),
            data,
            owner: spl_token::id(),
            executable: false,
            rent_epoch: u64::MAX,
        }
    }

    /// Creates a mock SPL Token Account (ATA) with the specified balance.
    pub fn mk_ata(mint: &Pubkey, user: &Pubkey, amount: u64) -> Account {
        let ata = spl_token::state::Account {
            mint: *mint,
            owner: *user,
            amount,
            state: spl_token::state::AccountState::Initialized,
            ..Default::default()
        };

        let mut data = vec![0u8; spl_token::state::Account::LEN];
        ata.pack_into_slice(&mut data);

        Account {
            lamports: Rent::default().minimum_balance(data.len()),
            data,
            owner: spl_token::id(),
            executable: false,
            rent_epoch: u64::MAX,
        }
    }

    /// Reads previously saved PMM accounts from disk.
    ///
    /// Searches the `accounts_path` directory for JSON files matching each DEX's prefix
    /// (e.g., `humidifi_*.json`) and deserialises them into account data.
    pub fn read_accounts_from_disk(pmms: &[Dex], accounts_path: &str) -> eyre::Result<(Option<u64>, HashMap<Dex, Vec<(Pubkey, Account)>>)> {
        let pmms: HashSet<_> = pmms.iter().collect();
        let mut res = HashMap::new();
        let mut all_slots: Vec<u64> = vec![];

        let data_dir = Path::new(accounts_path);
        if !data_dir.exists() {
            return Ok((None, res));
        }

        pmms.iter().try_for_each(|pmm| -> eyre::Result<()> {
            let prefix = pmm.to_string();
            let mut dex_accs = vec![];
            let mut slots = vec![];

            fs::read_dir(data_dir)?.into_iter().try_for_each(|entry| -> eyre::Result<()> {
                let entry = entry?;
                let path = entry.path();

                if path.is_file()
                    && path.file_name().and_then(|n| n.to_str()).is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"))
                {
                    let (slot, pubkey, acc) = Misc::parse_account_from_file(&path)?;
                    dex_accs.push((pubkey, acc));
                    if let Some(s) = slot {
                        slots.push(s);
                    }
                }

                Ok(())
            })?;

            if !slots.is_empty() {
                all_slots.extend(&slots);
            }

            res.insert(**pmm, dex_accs);
            info!("loaded accounts for {pmm} from disk");
            Ok(())
        })?;

        let slot = if all_slots.is_empty() {
            None
        } else {
            let first_slot = all_slots[0];
            if all_slots.iter().any(|&s| s != first_slot) {
                let min_slot = all_slots.iter().min().copied().unwrap();
                let max_slot = all_slots.iter().max().copied().unwrap();
                warn!("slot mismatch across PMMs: accounts fetched at different slots ({min_slot} - {max_slot}), using {first_slot}");
            }
            Some(first_slot)
        };

        Ok((slot, res))
    }

    /// Persists an account to disk in JSON for later reuse.
    pub fn save_account_to_disk(accounts_path: &str, dex: &Dex, pubkey: &Pubkey, account: &Account, slot: u64) -> eyre::Result<()> {
        let filename = format!("{}_{}.json", dex, pubkey);
        let data_dir = Path::new(&accounts_path);

        if !data_dir.exists() {
            fs::create_dir_all(data_dir)?;
        }

        let file_path = data_dir.join(filename);

        let value = serde_json::json!({
            "pubkey": pubkey.to_string(),
            "slot": slot,
            "account": {
                "lamports": account.lamports,
                "data": [general_purpose::STANDARD.encode(&account.data), "base64"],
                "owner": account.owner.to_string(),
                "executable": account.executable,
                "rentEpoch": account.rent_epoch,
            }
        });

        let mut file = File::create(file_path)?;
        file.write_all(serde_json::to_string_pretty(&value)?.as_bytes())?;

        Ok(())
    }

    /// Fetches PMM accounts from an RPC node in a single atomic request.
    ///
    /// Collects all account pubkeys from the provided DEX configurations and fetches
    /// them in one `get_multiple_accounts_with_commitment` call to ensure all accounts
    /// are read at the same slot.
    pub fn fetch_accounts(pmms: &[Dex], client: &RpcClient, cfg: &PMMCfg) -> eyre::Result<(u64, HashMap<Dex, Vec<(Pubkey, Account)>>)> {
        let pmms: HashSet<_> = pmms.iter().collect();
        let mut res = HashMap::new();

        // track which dex the accounts belong to
        let mut all_pubkeys: Vec<Pubkey> = vec![];
        let mut dex_ranges: Vec<(Dex, std::ops::Range<usize>)> = vec![];

        pmms.iter().for_each(|pmm| {
            let Some(accs) = cfg.get_accounts(pmm) else {
                warn!("skipping unsupported prop amms: {pmm}");
                return;
            };

            let start = all_pubkeys.len();
            all_pubkeys.extend(accs.iter());
            let end = all_pubkeys.len();
            dex_ranges.push((**pmm, start..end));
        });

        let response = client.get_multiple_accounts_with_commitment(&all_pubkeys, CommitmentConfig::confirmed())?;
        let slot = response.context.slot;
        let all_accs = response.value;

        info!("fetched {} accounts for {pmms:?} at slot {slot}", all_pubkeys.len());

        // reconstruct per-dex account maps
        dex_ranges.iter().for_each(|(dex, range)| {
            let mut dex_accs = vec![];

            all_pubkeys[range.clone()].iter().enumerate().for_each(|(i, pubkey)| {
                let idx = range.start + i;
                if let Some(acc) = &all_accs[idx] {
                    dex_accs.push((*pubkey, acc.clone()));
                } else {
                    warn!("account {pubkey} not found for {dex}");
                }
            });

            res.insert(*dex, dex_accs);
        });

        Ok((slot, res))
    }

    /// Parses a Solana account from a JSON file.
    ///
    /// Expected JSON format (matches Solana CLI `account` command output):
    /// ```json
    /// {
    ///   "slot": 12345678,
    ///   "pubkey": "Base58EncodedPubkey",
    ///   "account": {
    ///     "lamports": 1000000,
    ///     "data": ["Base64EncodedData", "base64"],
    ///     "owner": "Base58EncodedOwner",
    ///     "executable": false,
    ///     "rentEpoch": 0
    ///   }
    /// }
    /// ```
    ///
    /// # Returns
    /// A tuple of (slot, pubkey, account).
    pub fn parse_account_from_file(path: &Path) -> eyre::Result<(Option<u64>, Pubkey, Account)> {
        let contents = fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&contents)?;

        let pubkey = Pubkey::from_str(value["pubkey"].as_str().ok_or_else(|| eyre::eyre!("missing pubkey"))?)?;
        let lamports = value["account"]["lamports"].as_u64().ok_or_else(|| eyre::eyre!("missing lamports"))?;
        let data_base64 = value["account"]["data"][0].as_str().ok_or_else(|| eyre::eyre!("missing data"))?;
        let data = general_purpose::STANDARD.decode(data_base64)?;
        let owner = Pubkey::from_str(value["account"]["owner"].as_str().ok_or_else(|| eyre::eyre!("missing owner"))?)?;
        let executable = value["account"]["executable"].as_bool().ok_or_else(|| eyre::eyre!("missing executable"))?;
        let rent_epoch = value["account"]["rentEpoch"].as_u64().ok_or_else(|| eyre::eyre!("missing rentEpoch"))?;
        let slot = value["slot"].as_u64();

        Ok((slot, pubkey, Account { lamports, data, owner, executable, rent_epoch }))
    }

    /// Fetches program ELF bytes from RPC for the given PMMs.
    ///
    /// For each *upgradeable* program, resolves the programdata account and strips the
    /// 45-byte header to extract the raw ELF bytecode.
    pub fn fetch_programs(pmms: &[Dex], client: &RpcClient) -> eyre::Result<Vec<(Dex, Pubkey, Vec<u8>)>> {
        let mut programs = vec![];

        pmms.iter().try_for_each(|pmm| -> eyre::Result<()> {
            let program_id = Pubkey::new_from_array(pmm.program_id().to_bytes());
            let acc = client.get_account(&program_id)?;

            // upgradeable programs: 4-byte tag + 32-byte programdata address
            // https://github.com/solana-labs/solana/blob/master/sdk/program/src/bpf_loader_upgradeable.rs#L70-L73
            let programdata_pubkey = Pubkey::new_from_array(acc.data[4..36].try_into()?);
            let programdata_acc = client.get_account(&programdata_pubkey)?;

            // strip 45-byte programdata header (tag + slot + upgrade authority)
            // https://github.com/solana-labs/solana/blob/master/cli/src/program.rs#L1861C66-L1862
            let elf_bytes = programdata_acc.data[45..].to_vec();
            programs.push((*pmm, program_id, elf_bytes));

            info!("fetched program {pmm} ({program_id})");
            Ok(())
        })?;

        Ok(programs)
    }

    /// Reads program .so files from disk for the given PMMs.
    pub fn read_programs_from_disk(pmms: &[Dex], programs_path: &str) -> eyre::Result<Vec<(Pubkey, String)>> {
        let mut programs = vec![];

        pmms.iter().try_for_each(|pmm| -> eyre::Result<()> {
            let program_id = Pubkey::new_from_array(pmm.program_id().to_bytes());
            let path = format!("{}/{}.so", programs_path, pmm);
            programs.push((program_id, path));
            Ok(())
        })?;

        Ok(programs)
    }

    /// Custom serde deserializer for `Pubkey` from a base58-encoded string.
    ///
    /// Used with `#[serde(deserialize_with = "Misc::deserialize_pubkey")]` attribute
    /// on struct fields that should be deserialized as Solana pubkeys.
    pub fn deserialize_pubkey<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Pubkey::from_str(&s).map_err(serde::de::Error::custom)
    }

    /// Converts a raw token amount (in base units) to a human-readable decimal value.
    pub fn to_human(amount: u64, dec: u8) -> f64 {
        amount as f64 / 10f64.powi(dec as i32)
    }

    /// Converts a human-readable decimal value to raw token amount (in base units).
    pub fn to_raw(amount: f64, dec: u8) -> u64 {
        (amount * 10f64.powi(dec as i32)) as u64
    }

    pub fn gen_order_id() -> u64 {
        SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }
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
            Column::new("compute_units".into(), self.records.iter().map(|r| r.compute_units).collect::<Vec<_>>()),
        ])?;

        let mut file = File::create(&self.save_path)?;
        ParquetWriter::new(&mut file).finish(&mut df.clone())?;

        Ok(())
    }
}

pub struct App {
    args: CliArgs,
    cfg: PMMCfg,
}

impl App {
    pub fn new(args: CliArgs, cfg: PMMCfg) -> Self {
        Self { args, cfg }
    }

    pub fn start(&self) -> eyre::Result<()> {
        match &self.args.command {
            Command::FetchAccounts { .. } => self.fetch_accounts(),
            Command::FetchPrograms { .. } => self.fetch_programs(),
            Command::Benchmark { .. } => self.benchmark(),
            Command::Single { .. } | Command::Multi { .. } => self.simulate(),
        }
    }

    pub fn fetch_accounts(&self) -> eyre::Result<()> {
        let Command::FetchAccounts { http_url, accounts_path, pmms, .. } = &self.args.command else { unreachable!() };

        let rpc_client = RpcClient::new(http_url.expose_secret().to_string());
        let (slot, fetched) = Misc::fetch_accounts(pmms, &rpc_client, &self.cfg)?;

        fetched.iter().try_for_each(|(dex, accs)| -> eyre::Result<()> {
            accs.iter().try_for_each(|(pubkey, acc)| -> eyre::Result<()> {
                Misc::save_account_to_disk(accounts_path, dex, pubkey, acc, slot)?;
                info!("saved account {pubkey} for {dex}");
                Ok(())
            })?;

            Ok(())
        })?;

        info!("done fetching accounts at slot {slot}");
        Ok(())
    }

    pub fn fetch_programs(&self) -> eyre::Result<()> {
        let Command::FetchPrograms { http_url, programs_path, pmms, .. } = &self.args.command else { unreachable!() };

        let rpc_client = RpcClient::new(http_url.expose_secret().to_string());
        let programs = Misc::fetch_programs(pmms, &rpc_client)?;

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
        let Command::Benchmark { common, datasets_path, pmms, range } = &self.args.command else { unreachable!() };

        let rpc_client = Arc::new(RpcClient::new(common.http_url.expose_secret().to_string()));
        let (src_mint, src_dec, src_name) = (common.src_token.get_addr(), common.src_token.get_decimals(), common.src_token.to_string());
        let (dst_mint, dst_dec, dst_name) = (common.dst_token.get_addr(), common.dst_token.get_decimals(), common.dst_token.to_string());
        let mints = vec![(src_mint, src_dec), (dst_mint, dst_dec)];

        let benchmark = Benchmark::new().range_from_human(*range, src_dec);
        let multi = MultiProgress::new();

        let (slot, accs_map) = if common.jit_accounts {
            let (s, m) = Misc::fetch_accounts(pmms, &rpc_client.clone(), &self.cfg)?;
            (Some(s), m)
        } else {
            Misc::read_accounts_from_disk(pmms, &common.accounts_path)?
        };

        thread::scope(|s| {
            let handles: Vec<_> = pmms
                .iter()
                .map(|pmm| {
                    let (cfg, multi, mints) = (&self.cfg, &multi, &mints);
                    let (src_name, dst_name) = (&src_name, &dst_name);
                    let rpc_client = rpc_client.clone();
                    let pmm_accs = accs_map.get(pmm).cloned().unwrap_or_default();
                    let benchmark = benchmark.clone();

                    s.spawn(move || -> eyre::Result<()> {
                        // start up the progress bar only when all the spawned threads
                        // have finished bootstrapping so there's no CLI progress bar race cond
                        let (mut env, src_ata, dst_ata) = multi.suspend(|| -> eyre::Result<_> {
                            let mut env = Environment::new(&common.programs_path, &common.accounts_path, Some(mints), cfg.clone(), slot)?;
                            env.get_and_load_programs(&[*pmm], common.jit_programs, Some(&rpc_client))?.setup_wallet(
                                &src_mint,
                                benchmark.range_end(),
                                consts::AIRDROP_AMOUNT,
                            )?;

                            let (src_ata, dst_ata) = (env.wallet_ata(&src_mint), env.wallet_ata(&dst_mint));
                            Ok((env, src_ata, dst_ata))
                        })?;

                        let market = cfg.get_market(pmm).unwrap_or_else(|| panic!("{} not configured", pmm)).to_string();

                        let pb = multi.add(ProgressBar::new(benchmark.range_count()));
                        pb.set_style(
                            ProgressStyle::default_bar().template(consts::PROGRESS_TEMPLATE)?.progress_chars(consts::PROGRESS_CHARS),
                        );
                        pb.set_prefix(format!("{}", pmm));

                        let (mut records, mut warn_cnt) = (vec![], u64::default());
                        let routes: Vec<Vec<magnus_router_client::types::Route>> =
                            vec![vec![Route { dexes: vec![*pmm], weights: vec![100] }.into()]];

                        benchmark.range_iter().try_for_each(|amount_in| -> eyre::Result<()> {
                            env.reset_wallet(&src_mint, amount_in)?;
                            env.set_accounts(&pmm_accs)?;

                            let mut swap_builder = SwapBuilder::new()
                                .payer(env.wallet_pubkey())
                                .source_token_account(src_ata)
                                .destination_token_account(dst_ata)
                                .source_mint(src_mint)
                                .destination_mint(dst_mint)
                                .amount_in(amount_in)
                                .expect_amount_out(1)
                                .min_return(1)
                                .amounts(vec![amount_in])
                                .routes(routes.clone())
                                .order_id(Misc::gen_order_id())
                                .clone();

                            let swap_ix = ConstructSwap {
                                cfg: cfg.clone(),
                                builder: &mut swap_builder,
                                payer: env.wallet_pubkey(),
                                src_ta: src_ata,
                                dst_ta: dst_ata,
                                src_mint,
                                dst_mint,
                            }
                            .attach_pmm_accs(pmm)
                            .instruction();

                            let tx = Transaction::new_signed_with_payer(
                                &[swap_ix],
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

                            let amount_out = env.get_event_amount_out(&res);

                            records.push(BenchmarkRecord {
                                slot: env.slot.unwrap_or_default(),
                                pmm: pmm.to_string(),
                                market: market.clone(),
                                src_token: src_name.clone(),
                                dst_token: dst_name.clone(),
                                amount_in: Misc::to_human(amount_in, src_dec),
                                amount_out: Misc::to_human(amount_out, dst_dec),
                                compute_units: res.compute_units_consumed,
                            });

                            pb.set_message(format!("in: {:.2}", Misc::to_human(amount_in, src_dec)));
                            pb.inc(1);

                            Ok(())
                        })?;

                        (warn_cnt != 0).then(|| pb.println(format!("[WARN] {}: {} total failures", pmm, warn_cnt)));

                        let filename =
                            format!("{}/{}_{}_{}_{}.parquet", datasets_path, env.slot.unwrap_or_default(), pmm, market, benchmark.time());
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
        let (common, amount_in, pmms, weights) = match &self.args.command {
            Command::Single { common, amount_in, pmms, weights } => (common, vec![*amount_in], vec![pmms.clone()], vec![weights.clone()]),
            Command::Multi { common, amount_in, pmms, weights } => {
                let pmms = CliArgs::parse_nested_pmms(pmms).expect("invalid format for nested dexes");
                let weights = CliArgs::parse_nested_weights(weights).expect("invalid format for nested weights");

                (common, amount_in.clone(), pmms, weights)
            }
            _ => unreachable!(),
        };

        // ensure that each dex has a corresponding weight
        assert_eq!(pmms.len(), weights.len(), "dexes and weights outer length mismatch");
        pmms.iter().zip(weights.iter()).for_each(|(d, w)| {
            assert_eq!(d.len(), w.len(), "dexes and weights length mismatch");
        });

        let rpc_client = RpcClient::new(common.http_url.expose_secret().to_string());
        let flat_pmms: Vec<Dex> = pmms.iter().flatten().copied().collect();

        let (src_mint, src_dec, src_name) = (common.src_token.get_addr(), common.src_token.get_decimals(), common.src_token.to_string());
        let (dst_mint, dst_dec, dst_name) = (common.dst_token.get_addr(), common.dst_token.get_decimals(), common.dst_token.to_string());
        let mints = vec![(src_mint, src_dec), (dst_mint, dst_dec)];

        let amount_in: Vec<u64> = amount_in.iter().map(|amount| Misc::to_raw(*amount, src_dec)).collect();
        let amount_in_sum: u64 = amount_in.iter().sum();

        let mut env = Environment::new(&common.programs_path, &common.accounts_path, Some(&mints), self.cfg.clone(), None)?;
        env.get_and_load_programs(&flat_pmms, common.jit_programs, Some(&rpc_client))?
            .get_and_load_accounts(&flat_pmms, common.jit_accounts, Some(&rpc_client))?
            .setup_wallet(&src_mint, amount_in_sum, consts::AIRDROP_AMOUNT)?;
        info!(?env);

        let (src_ata, src_before) = (env.wallet_ata(&src_mint), env.token_balance_norm(&src_mint, src_dec));
        let (dst_ata, dst_before) = (env.wallet_ata(&dst_mint), env.token_balance_norm(&dst_mint, dst_dec));

        debug!(?src_name, ?src_before, ?dst_name, ?dst_before);

        let routes: Vec<Vec<magnus_router_client::types::Route>> = pmms
            .iter()
            .zip(weights.iter())
            .map(|(dex_grp, weight_group)| vec![Route { dexes: dex_grp.clone(), weights: weight_group.clone() }.into()])
            .collect();

        let mut swap_builder = SwapBuilder::new()
            .payer(env.wallet_pubkey())
            .source_token_account(src_ata)
            .destination_token_account(dst_ata)
            .source_mint(src_mint)
            .destination_mint(dst_mint)
            .amount_in(amount_in_sum)
            .expect_amount_out(1)
            .min_return(1)
            .amounts(amount_in)
            .routes(routes.clone())
            .order_id(Misc::gen_order_id())
            .clone();

        let swap_ix = ConstructSwap {
            cfg: self.cfg.clone(),
            builder: &mut swap_builder,
            payer: env.wallet_pubkey(),
            src_ta: src_ata,
            dst_ta: dst_ata,
            src_mint,
            dst_mint,
        }
        .attach_pmms_accs(&flat_pmms)
        .instruction();

        let tx = Transaction::new_signed_with_payer(&[swap_ix], Some(&env.wallet_pubkey()), &[&env.wallet], env.latest_blockhash());
        let res = env.send_transaction(tx).expect("failed to exec tx");
        let amount_out = env.get_event_amount_out(&res);

        info!(
            src_token = %src_name,
            dst_token = %dst_name,
            routes = ?routes,
            amount_in = ?Misc::to_human(amount_in_sum, src_dec),
            amount_out = ?Misc::to_human(amount_out, dst_dec),
            cu = res.compute_units_consumed
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod cli {
        use super::*;
        #[test]
        fn test_parse_nested_pmms_json_single() {
            let input = r#"[["humidifi"]]"#;
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::HumidiFi]]);
        }

        #[test]
        fn test_parse_nested_pmms_json_multiple() {
            let input = r#"[["humidifi","obric-v2"],["zerofi"]]"#;
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::HumidiFi, Dex::ObricV2], vec![Dex::ZeroFi]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_single() {
            let input = "[[humidifi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::HumidiFi]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_single_route_multiple_pmms() {
            let input = "[[humidifi,obric-v2]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::HumidiFi, Dex::ObricV2]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_multiple_routes() {
            let input = "[[humidifi,obric-v2],[zerofi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::HumidiFi, Dex::ObricV2], vec![Dex::ZeroFi]]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_three_routes() {
            let input = "[[humidifi],[obric-v2,solfi-v2],[zerofi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::HumidiFi], vec![Dex::ObricV2, Dex::SolfiV2], vec![Dex::ZeroFi],]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_all_pmms() {
            let input = "[[raydium-cl-v2,raydium-cp],[obric-v2,solfi-v2,zerofi,humidifi]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::RaydiumClV2, Dex::RaydiumCp], vec![Dex::ObricV2, Dex::SolfiV2, Dex::ZeroFi, Dex::HumidiFi],]);
        }

        #[test]
        fn test_parse_nested_pmms_no_quotes_with_spaces() {
            let input = "[[ humidifi , obric-v2 ],[ zerofi ]]";
            let result = CliArgs::parse_nested_pmms(input).unwrap();
            assert_eq!(result, vec![vec![Dex::HumidiFi, Dex::ObricV2], vec![Dex::ZeroFi]]);
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

    mod environment {
        use super::*;

        fn default_cfg() -> PMMCfg {
            PMMCfg::default()
        }

        #[test]
        fn test_new_generates_unique_wallet() {
            let env1 = Environment::new("", "", None, default_cfg(), None).unwrap();
            let env2 = Environment::new("", "", None, default_cfg(), None).unwrap();

            assert_ne!(env1.wallet_pubkey(), env2.wallet_pubkey());
        }

        #[test]
        fn test_new_without_slot_leaves_slot_none() {
            let env = Environment::new("", "", None, default_cfg(), None).unwrap();

            assert!(env.slot.is_none());
        }

        #[test]
        fn test_new_with_slot_sets_slot() {
            let slot = 12345678u64;
            let env = Environment::new("", "", None, default_cfg(), Some(slot)).unwrap();

            assert_eq!(env.slot, Some(slot));
        }

        #[test]
        fn test_new_without_mints_leaves_mints_none() {
            let env = Environment::new("", "", None, default_cfg(), None).unwrap();

            assert!(env.mints.is_none());
        }

        #[test]
        fn test_new_with_mints_creates_mint_accounts() {
            let mints = vec![(consts::WSOL, consts::WSOL_DECIMALS), (consts::USDC, consts::USDC_DECIMALS)];

            let env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

            assert!(env.mints.is_some());
            assert_eq!(env.mints.unwrap().len(), 2);

            // verify mint accounts exist in SVM
            let wsol_account = env.svm.get_account(&consts::WSOL);
            let usdc_account = env.svm.get_account(&consts::USDC);

            assert!(wsol_account.is_some());
            assert!(usdc_account.is_some());

            // verify mint account data is valid
            let wsol_mint = spl_token::state::Mint::unpack(&wsol_account.unwrap().data).unwrap();
            let usdc_mint = spl_token::state::Mint::unpack(&usdc_account.unwrap().data).unwrap();

            assert_eq!(wsol_mint.decimals, consts::WSOL_DECIMALS);
            assert_eq!(usdc_mint.decimals, consts::USDC_DECIMALS);
            assert!(wsol_mint.is_initialized);
            assert!(usdc_mint.is_initialized);
        }

        #[test]
        fn test_wallet_ata_derives_correct_address() {
            let mints = vec![(consts::WSOL, consts::WSOL_DECIMALS)];
            let env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

            let expected_ata = get_associated_token_address(&env.wallet_pubkey(), &consts::WSOL);
            let actual_ata = env.wallet_ata(&consts::WSOL);

            assert_eq!(actual_ata, expected_ata);
        }

        #[test]
        fn test_setup_wallet_creates_atas_and_funds() {
            let mints = vec![(consts::WSOL, consts::WSOL_DECIMALS), (consts::USDC, consts::USDC_DECIMALS)];
            let mut env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

            let src_amount = 1_000_000_000u64; // 1 SOL
            let airdrop = 10_000_000_000u64; // 10 SOL for fees

            env.setup_wallet(&consts::WSOL, src_amount, airdrop).unwrap();

            // verify src token balance is correct
            assert_eq!(env.token_balance(&consts::WSOL), src_amount);

            // verify dst token balance is zero
            assert_eq!(env.token_balance(&consts::USDC), 0);

            // verify SOL was airdropped
            let wallet_account = env.svm.get_account(&env.wallet_pubkey());
            assert!(wallet_account.is_some());
            assert!(wallet_account.unwrap().lamports >= airdrop);
        }

        #[test]
        fn test_reset_wallet_restores_balances() {
            let mints = vec![(consts::WSOL, consts::WSOL_DECIMALS), (consts::USDC, consts::USDC_DECIMALS)];
            let mut env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

            env.setup_wallet(&consts::WSOL, 1_000_000_000, 10_000_000_000).unwrap();

            // simulate a swap by manually changing balances
            let wsol_ata = env.wallet_ata(&consts::WSOL);
            let usdc_ata = env.wallet_ata(&consts::USDC);
            env.svm.set_account(wsol_ata, Misc::mk_ata(&consts::WSOL, &env.wallet_pubkey(), 500_000_000)).unwrap();
            env.svm.set_account(usdc_ata, Misc::mk_ata(&consts::USDC, &env.wallet_pubkey(), 100_000_000)).unwrap();

            // seset wallet
            let new_amount = 2_000_000_000u64;
            env.reset_wallet(&consts::WSOL, new_amount).unwrap();

            // verify balances are reset
            assert_eq!(env.token_balance(&consts::WSOL), new_amount);
            assert_eq!(env.token_balance(&consts::USDC), 0);
        }

        #[test]
        fn test_token_balance_returns_zero_for_nonexistent_ata() {
            let env = Environment::new("", "", None, default_cfg(), None).unwrap();

            assert_eq!(env.token_balance(&consts::WSOL), 0);
        }

        #[test]
        fn test_token_balance_norm_converts_correctly() {
            let mints = vec![(consts::WSOL, consts::WSOL_DECIMALS)];
            let mut env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

            let raw_amount = 1_500_000_000u64; // 1.5 SOL in lamports
            env.setup_wallet(&consts::WSOL, raw_amount, 10_000_000_000).unwrap();

            let normalized = env.token_balance_norm(&consts::WSOL, consts::WSOL_DECIMALS);

            assert!((normalized - 1.5).abs() < f64::EPSILON);
        }

        #[test]
        fn test_latest_blockhash_returns_valid_hash() {
            let env = Environment::new("", "", None, default_cfg(), None).unwrap();

            let blockhash = env.latest_blockhash();

            // should not be the default/zero hash
            assert_ne!(blockhash, solana_sdk::hash::Hash::default());
        }

        #[test]
        fn test_load_accounts_sets_accounts_in_svm() {
            let mut env = Environment::new("", "", None, default_cfg(), None).unwrap();

            let pubkey = Pubkey::new_unique();
            let account =
                Account { lamports: 1_000_000, data: vec![1, 2, 3, 4], owner: Pubkey::new_unique(), executable: false, rent_epoch: 0 };

            env.set_accounts(&vec![(pubkey, account.clone())]).unwrap();

            let loaded = env.svm.get_account(&pubkey).unwrap();
            assert_eq!(loaded.lamports, account.lamports);
            assert_eq!(loaded.data, account.data);
            assert_eq!(loaded.owner, account.owner);
        }
    }

    mod misc {
        use super::*;

        #[test]
        fn test_to_human_wsol_decimals() {
            let lamports = 1_500_000_000u64;
            let sol = Misc::to_human(lamports, 9);
            assert_eq!(sol, 1.5);
        }

        #[test]
        fn test_to_human_usdc_decimals() {
            let raw = 1_500_000u64;
            let usdc = Misc::to_human(raw, 6);
            assert_eq!(usdc, 1.5);
        }

        #[test]
        fn test_to_human_zero() {
            assert_eq!(Misc::to_human(0, 9), 0.0);
        }

        #[test]
        fn test_to_raw_wsol_decimals() {
            let sol = 1.5f64;
            let lamports = Misc::to_raw(sol, 9);
            assert_eq!(lamports, 1_500_000_000);
        }

        #[test]
        fn test_to_raw_usdc_decimals() {
            let usdc = 1.5f64;
            let raw = Misc::to_raw(usdc, 6);
            assert_eq!(raw, 1_500_000);
        }

        #[test]
        fn test_to_raw_zero() {
            assert_eq!(Misc::to_raw(0.0, 9), 0);
        }

        #[test]
        fn test_to_human_to_raw_roundtrip() {
            let original = 123_456_789u64;
            let decimals = 9u8;

            let human = Misc::to_human(original, decimals);
            let back = Misc::to_raw(human, decimals);

            assert_eq!(back, original);
        }

        #[test]
        fn test_mk_mint_acc_creates_valid_mint() {
            let decimals = 9u8;
            let account = Misc::mk_mint_acc(decimals);

            assert_eq!(account.owner, spl_token::id());
            assert!(!account.executable);

            let mint = spl_token::state::Mint::unpack(&account.data).unwrap();
            assert_eq!(mint.decimals, decimals);
            assert!(mint.is_initialized);
            assert_eq!(mint.supply, u64::MAX);
            assert!(mint.mint_authority.is_none());
            assert!(mint.freeze_authority.is_none());
        }

        #[test]
        fn test_mk_mint_acc_different_decimals() {
            [0, 6, 9, 18].iter().for_each(|decimals| {
                let account = Misc::mk_mint_acc(*decimals);
                let mint = spl_token::state::Mint::unpack(&account.data).unwrap();
                assert_eq!(mint.decimals, *decimals);
            });
        }

        #[test]
        fn test_mk_ata_creates_valid_token_account() {
            let mint = Pubkey::new_unique();
            let owner = Pubkey::new_unique();
            let amount = 1_000_000u64;

            let account = Misc::mk_ata(&mint, &owner, amount);

            // verify owner is token program
            assert_eq!(account.owner, spl_token::id());

            // verify data is valid token account
            let token_acc = spl_token::state::Account::unpack(&account.data).unwrap();
            assert_eq!(token_acc.mint, mint);
            assert_eq!(token_acc.owner, owner);
            assert_eq!(token_acc.amount, amount);
            assert_eq!(token_acc.state, spl_token::state::AccountState::Initialized);
        }

        #[test]
        fn test_mk_ata_zero_balance() {
            let mint = Pubkey::new_unique();
            let owner = Pubkey::new_unique();

            let account = Misc::mk_ata(&mint, &owner, 0);
            let token_acc = spl_token::state::Account::unpack(&account.data).unwrap();

            assert_eq!(token_acc.amount, 0);
        }
    }
}
