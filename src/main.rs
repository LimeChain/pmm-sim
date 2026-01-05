//! Simulation & Benchmark environment for Solana's Proprietary AMMs.
//!
//! Simulate and/or Benchmark swaps across *any* of the major Solana Proprietary AMMs, locally, using LiteSVM.
#![doc = include_str!("../README.md")]
#![allow(clippy::type_complexity, clippy::result_large_err)]
#![deny(unused)]

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
use csv::Writer;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use litesvm::{LiteSVM, types::TransactionMetadata};
use magnus_router_client::instructions::SwapBuilder;
use magnus_shared::{Dex, Route};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_sdk::{
    account::Account, message::AccountMeta, program_pack::Pack, pubkey::Pubkey, rent::Rent, signature::Keypair, signer::Signer, sysvar,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use tracing::{debug, info, warn};
use tracing_subscriber::{EnvFilter, fmt::time::UtcTime};

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

    pub const PROGRESS_TEMPLATE: &str = "{prefix:>12.bold} [{bar:40.cyan/blue}] {pos:>6}/{len:<6} ({percent}%)";
    pub const PROGRESS_CHARS: &str = "█▓░";

    // used to pay for tx fees
    pub const AIRDROP_AMOUNT: u64 = 1_000_000_000;
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
    Humidifi => HumidifiCfg : humidifi ("humidifi") {
        market,
        base_token_acc,
        quote_token_acc,
    },
    Tessera => TesseraCfg : tessera ("tessera") {
        market,
        base_token_acc,
        quote_token_acc,
        global_state,
    },
    Goonfi => GoonfiCfg : goonfi ("goonfi") {
        market,
        base_token_acc,
        quote_token_acc,
        blacklist,
    },
    SolfiV2 => SolfiV2Cfg : solfi_v2 ("solfi-v2") {
        market,
        base_token_acc,
        quote_token_acc,
        cfg,
        oracle,
    },
    Zerofi => ZerofiCfg : zerofi ("zerofi") {
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

    fn parse_steps(s: &str) -> Result<[f64; 3], String> {
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
  # Single swap with one Prop AMM
  pmm-sim multi --pmms='[[humidifi]]' --weights='[[100]]'

  # Two sequential swaps: (Humidifi + Obric) followed by Zerofi
  pmm-sim multi --pmms='[[humidifi,zerofi],[solfi-v2]]' --weights='[[50,50],[100]]'

  # Complex three-step nested route
  pmm-sim multi --amount-in=10 --pmms='[[humidifi,tessera],[solfi-v2],[goonfi,humidifi]]' --weights='[[100],[60,40],[95,5]]'"
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
        about = "Benchmark swaps for any one of the implemented Prop AMMs by specifying, optionally, the accounts, src/dst tokens and \
                 step size",
        after_help = "Examples:
  # Benchmark Humidifi swaps (WSOL->USDC) with the current AMM state, stepping from 1 to 100 with a step size of 1. The resulting CSV
  # will be saved in the ./datasets directory
  pmm-sim benchmark --pmms=humidifi --steps=1.0,100.0,1.0

  # Benchmark SolfiV2 and Tessera swaps (WSOL->USDC) with the current AMM state, stepping from 10 to 1000 with a step size of 5. The
  # resulting CSVs will be saved in the ./datasets directory
  pmm-sim benchmark --pmms=solfi-v2,tessera --src-token=WSOL --dst-token=USDC --steps=10.0,1000.0,5.0
        "
    )]
    Benchmark {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(long, env = "DATASETS_PATH", default_value = consts::DATASETS_PATH, help = "Directory to dump the benchmark CSVs into")]
        datasets_path: String,

        #[arg(long, env = "PROP_AMMS", value_delimiter = ',', default_value = "humidifi", help = "The Prop AMMs to benchmark")]
        pmms: Vec<Dex>,

        #[arg(long, env = "STEPS", default_value = "1.0,100.0,1.0", value_parser = CliArgs::parse_steps, help = "Comma-separated step parameters: start, end, step")]
        steps: [f64; 3],
    },
}

impl Command {
    fn setup_path(&self) -> &str {
        match self {
            Command::FetchAccounts { setup_path, .. } => setup_path,
            Command::Benchmark { common, .. } => &common.setup_path,
            Command::Single { common, .. } => &common.setup_path,
            Command::Multi { common, .. } => &common.setup_path,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Command::FetchAccounts { .. } => "FetchAccounts",
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
struct Environment<'a, P: Into<String> + Display + Clone + Debug> {
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
    fn new(
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
            for (mint, mint_decimals) in mints {
                svm.set_account(*mint, Misc::mk_mint_acc(*mint_decimals))?;
            }
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
    fn setup_wallet(&mut self, src_mint: &Pubkey, src_amount: u64, airdrop_amount: u64) -> eyre::Result<()> {
        if let Some(mints) = self.mints {
            for (mint, _) in mints {
                let ata = self.wallet_ata(mint);
                let amount = if mint == src_mint { src_amount } else { 0 };
                self.svm.set_account(ata, Misc::mk_ata(mint, &self.wallet_pubkey(), amount))?;
            }
        }

        self.svm.airdrop(&self.wallet_pubkey(), airdrop_amount).expect("airdrop failed");

        Ok(())
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
    fn reset_wallet(&mut self, src_mint: &Pubkey, src_amount: u64) -> eyre::Result<()> {
        if let Some(mints) = self.mints {
            for (mint, _) in mints {
                let ata = self.wallet_ata(mint);
                let amount = if mint == src_mint { src_amount } else { 0 };
                self.svm.set_account(ata, Misc::mk_ata(mint, &self.wallet_pubkey(), amount))?;
            }
        }

        Ok(())
    }

    fn wallet_pubkey(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    fn wallet_ata(&self, mint: &Pubkey) -> Pubkey {
        get_associated_token_address(&self.wallet.pubkey(), mint)
    }

    /// Loads the router program and all required PMM programs into the SVM.
    ///
    /// Programs are loaded from `.so` files in the configured `programs_path` directory.
    /// The router program is always loaded, plus any unique PMM programs from the provided list.
    fn load_programs(&mut self, pmms: &[Dex]) -> eyre::Result<()> {
        // mandatory load
        self.svm
            .add_program_from_file(magnus_router_client::programs::ROUTER_ID, format!("{}/{}.so", self.programs_path, consts::ROUTER))?;

        let unique_pmms: HashSet<_> = pmms.iter().collect();
        for dex in unique_pmms {
            let program_id = dex.program_id();

            self.svm.add_program_from_file(Pubkey::new_from_array(program_id.to_bytes()), format!("{}/{}.so", self.programs_path, dex))?;
        }

        info!("loaded {pmms:?} programs");

        Ok(())
    }

    /// Sets multiple accounts in the SVM state.
    fn load_accounts(&mut self, accs: &Vec<(Pubkey, Account)>) -> eyre::Result<()> {
        for (pubkey, acc) in accs {
            self.svm.set_account(*pubkey, acc.clone())?;
        }

        Ok(())
    }

    /// Fetches and loads PMM accounts either from RPC (JIT) or from disk cache.
    fn fetch_and_load_accounts(&mut self, pmms: &[Dex], jit: bool, client: Option<&RpcClient>) -> eyre::Result<()> {
        match jit {
            true => {
                let rpc_client = client.expect("RPC client is required for JIT account loading");
                self.jit_accounts(pmms, rpc_client)?;
            }
            false => {
                self.static_accounts(pmms)?;
            }
        }

        Ok(())
    }

    /// Loads PMM accounts from disk cache and warps to the cached slot.
    fn static_accounts(&mut self, pmms: &[Dex]) -> eyre::Result<()> {
        let (slot, accs_map) = Misc::read_accounts_disk(pmms, &self.accounts_path.to_string())?;

        for (_, accs) in accs_map {
            self.load_accounts(&accs)?;
        }

        info!("loaded {pmms:?} accounts");
        if let Some(s) = slot {
            self.svm.warp_to_slot(s);
            self.slot = Some(s);
        }

        Ok(())
    }

    /// Fetches PMM accounts from RPC and warps to the fetched slot.
    fn jit_accounts(&mut self, pmms: &[Dex], client: &RpcClient) -> eyre::Result<()> {
        let (slot, fetched) = Misc::fetch_pmm_accounts(pmms, client, &self.cfg)?;

        for (_, accs) in fetched {
            self.load_accounts(&accs)?;
        }

        info!("loaded {pmms:?} accounts");
        self.svm.warp_to_slot(slot);
        self.slot = Some(slot);

        Ok(())
    }

    /// Persists an account to disk in JSON format for later reuse.
    fn save_account_to_disk(&self, dex: &Dex, pubkey: &Pubkey, account: &Account, slot: u64) -> eyre::Result<()> {
        let filename = format!("{}_{}.json", dex, pubkey);
        let accounts_path = format!("{}", self.accounts_path);
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

    /// Returns the token balance for the wallet's ATA of the given mint.
    fn token_balance(&self, mint: &Pubkey) -> u64 {
        let ata = self.wallet_ata(mint);
        let account = self.svm.get_account(&ata).unwrap_or_default();
        spl_token::state::Account::unpack(&account.data).map(|a| a.amount).unwrap_or(0)
    }

    fn latest_blockhash(&self) -> solana_sdk::hash::Hash {
        self.svm.latest_blockhash()
    }

    fn send_transaction(&mut self, tx: Transaction) -> litesvm::types::TransactionResult {
        self.svm.send_transaction(tx)
    }

    /// Extracts the output amount from a swap transaction's logs.
    ///
    /// Parses the `SwapEvent` log emitted by the router program to find the
    /// `amount_out` value. Panics if the event is not found in the logs.
    fn get_event_amount_out(&self, metadata: &TransactionMetadata) -> u64 {
        let amount_out: u64 = metadata
            .logs
            .iter()
            .find_map(|log| {
                if log.contains("SwapEvent") {
                    // i.e.: "Program log: SwapEvent { dex: Humidifi, amount_in: 1000000000, amount_out: 121518066 }"
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
    sta: Pubkey,
    dta: Pubkey,
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
    fn attach_pmm_accs(&mut self, pmm: &Dex) {
        match pmm {
            Dex::Humidifi => self.attach_humidifi_accs(),
            Dex::SolfiV2 => self.attach_solfiv2_accs(),
            Dex::Zerofi => self.attach_zerofi_accs(),
            Dex::ObricV2 => self.attach_obric_v2_accs(),
            Dex::Tessera => self.attach_tessera_accs(),
            Dex::Goonfi => self.attach_goonfi_accs(),
            _ => {
                unimplemented!()
            }
        };
    }

    fn attach_solfiv2_accs(&mut self) {
        if let Some(cfg) = &self.cfg.solfi_v2 {
            self.builder.add_remaining_accounts(&vec![
                AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_solfi_v2::id().to_bytes()), false),
                AccountMeta::new(self.payer, true),
                AccountMeta::new(self.sta, false),
                AccountMeta::new(self.dta, false),
                AccountMeta::new(cfg.market, false),
                AccountMeta::new_readonly(cfg.oracle, false),
                AccountMeta::new_readonly(cfg.cfg, false),
                AccountMeta::new(cfg.base_token_acc, false),
                AccountMeta::new(cfg.quote_token_acc, false),
                AccountMeta::new_readonly(consts::WSOL, false),
                AccountMeta::new_readonly(consts::USDC, false),
                AccountMeta::new_readonly(spl_token::id(), false),
                AccountMeta::new_readonly(spl_token::id(), false),
                AccountMeta::new_readonly(sysvar::instructions::id(), false),
            ]);
        } else {
            panic!("SolfiV2 config is missing, cannot attach accounts.");
        }
    }

    fn attach_humidifi_accs(&mut self) {
        let Some(cfg) = &self.cfg.humidifi else {
            panic!("Humidifi config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&vec![
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_humidifi::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.sta, false),
            AccountMeta::new(self.dta, false),
            AccountMeta::new_readonly(Misc::create_humidifi_param(1500), false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.base_token_acc, false),
            AccountMeta::new(cfg.quote_token_acc, false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }

    fn attach_zerofi_accs(&mut self) {
        let Some(cfg) = &self.cfg.zerofi else {
            panic!("Zerofi config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&vec![
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_zerofi::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.sta, false),
            AccountMeta::new(self.dta, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.vault_info_base, false),
            AccountMeta::new(cfg.vault_base, false),
            AccountMeta::new(cfg.vault_info_quote, false),
            AccountMeta::new(cfg.vault_quote, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }

    fn attach_obric_v2_accs(&mut self) {
        let Some(cfg) = &self.cfg.obric_v2 else {
            panic!("ObricV2 config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&vec![
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_obric_v2::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.sta, false),
            AccountMeta::new(self.dta, false),
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

    fn attach_tessera_accs(&mut self) {
        let Some(cfg) = &self.cfg.tessera else {
            panic!("Tessera config is missing, cannot attach accounts.");
        };

        self.builder.add_remaining_accounts(&vec![
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_tessera::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.sta, false),
            AccountMeta::new(self.dta, false),
            AccountMeta::new_readonly(cfg.global_state, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.base_token_acc, false),
            AccountMeta::new(cfg.quote_token_acc, false),
            AccountMeta::new_readonly(self.src_mint, false),
            AccountMeta::new_readonly(self.dst_mint, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
        ]);
    }

    fn attach_goonfi_accs(&mut self) {
        let Some(cfg) = &self.cfg.goonfi else {
            panic!("Goonfi config is missing, cannot attach accounts.");
        };

        let goonfi_param = Pubkey::new_from_array([0u8; 32]);

        self.builder.add_remaining_accounts(&vec![
            AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_goonfi::id().to_bytes()), false),
            AccountMeta::new(self.payer, true),
            AccountMeta::new(self.sta, false),
            AccountMeta::new(self.dta, false),
            AccountMeta::new_readonly(goonfi_param, false),
            AccountMeta::new(cfg.market, false),
            AccountMeta::new(cfg.base_token_acc, false),
            AccountMeta::new(cfg.quote_token_acc, false),
            AccountMeta::new_readonly(cfg.blacklist, false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ]);
    }
}

struct Misc;
impl Misc {
    fn create_humidifi_param(swap_id: u64) -> Pubkey {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&swap_id.to_le_bytes());
        Pubkey::new_from_array(bytes)
    }

    /// Creates fully initialised mint account suitable for use in LiteSVM simulations.
    fn mk_mint_acc(decimals: u8) -> Account {
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
    fn mk_ata(mint: &Pubkey, user: &Pubkey, amount: u64) -> Account {
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
    fn read_accounts_disk(pmms: &[Dex], accounts_path: &str) -> eyre::Result<(Option<u64>, HashMap<Dex, Vec<(Pubkey, Account)>>)> {
        let pmms: HashSet<_> = pmms.iter().collect();
        let mut res = HashMap::new();
        let mut all_slots: Vec<u64> = vec![];

        let data_dir = Path::new(accounts_path);
        if !data_dir.exists() {
            return Ok((None, res));
        }

        for pmm in pmms {
            let prefix = pmm.to_string();
            let mut dex_accounts = vec![];
            let mut slots = vec![];

            for entry in fs::read_dir(data_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_file()
                    && path.file_name().and_then(|n| n.to_str()).is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"))
                {
                    let (slot, pubkey, account) = Misc::parse_account_from_file(&path)?;
                    dex_accounts.push((pubkey, account));
                    if let Some(s) = slot {
                        slots.push(s);
                    }
                }
            }

            if !slots.is_empty() {
                all_slots.extend(&slots);
            }

            res.insert(*pmm, dex_accounts);
            info!("loaded accounts for {pmm} from disk");
        }

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

    /// Fetches PMM accounts from an RPC node in a single atomic request.
    ///
    /// Collects all account pubkeys from the provided DEX configurations and fetches
    /// them in one `get_multiple_accounts_with_commitment` call to ensure all accounts
    /// are read at the same slot.
    fn fetch_pmm_accounts(pmms: &[Dex], client: &RpcClient, cfg: &PMMCfg) -> eyre::Result<(u64, HashMap<Dex, Vec<(Pubkey, Account)>>)> {
        let pmms: HashSet<_> = pmms.iter().collect();
        let mut res = HashMap::new();

        // track which dex the accounts belong to
        let mut all_pubkeys: Vec<Pubkey> = vec![];
        let mut dex_ranges: Vec<(Dex, std::ops::Range<usize>)> = vec![];

        for pmm in &pmms {
            let Some(accounts) = cfg.get_accounts(pmm) else {
                warn!("skipping unsupported prop amms: {pmm}");
                continue;
            };

            let start = all_pubkeys.len();
            all_pubkeys.extend(accounts.iter());
            let end = all_pubkeys.len();
            dex_ranges.push((**pmm, start..end));
        }

        let response = client.get_multiple_accounts_with_commitment(&all_pubkeys, CommitmentConfig::confirmed())?;
        let slot = response.context.slot;
        let all_accounts = response.value;

        info!("fetched {} accounts for {pmms:?} at slot {slot}", all_pubkeys.len());

        // reconstruct per-dex account maps
        for (dex, range) in dex_ranges {
            let mut dex_accounts = vec![];
            for (i, pubkey) in all_pubkeys[range.clone()].iter().enumerate() {
                let idx = range.start + i;
                if let Some(acc) = &all_accounts[idx] {
                    dex_accounts.push((*pubkey, acc.clone()));
                } else {
                    warn!("account {pubkey} not found for {dex}");
                }
            }
            res.insert(dex, dex_accounts);
        }

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
    fn parse_account_from_file(path: &Path) -> eyre::Result<(Option<u64>, Pubkey, Account)> {
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

    /// Custom serde deserializer for `Pubkey` from a base58-encoded string.
    ///
    /// Used with `#[serde(deserialize_with = "Misc::deserialize_pubkey")]` attribute
    /// on struct fields that should be deserialized as Solana pubkeys.
    fn deserialize_pubkey<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Pubkey::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Serialize, Clone)]
struct BenchmarkRecord<'a> {
    slot: u64,
    pmm: Dex,
    market: &'a str,
    src_token: &'a str,
    dst_token: &'a str,
    amount_in: f64,
    amount_out: f64,
    compute_units: u64,
}

#[derive(Debug)]
struct Benchmark<'a> {
    records: Vec<BenchmarkRecord<'a>>,
    writer: csv::Writer<File>,
}

impl<'a> Benchmark<'a> {
    pub fn new(records: Vec<BenchmarkRecord<'a>>, save_path: &str) -> eyre::Result<Self> {
        let save_path = if save_path.ends_with(".csv") {
            save_path.to_string()
        } else {
            format!("{}.csv", save_path)
        };

        let writer = Writer::from_path(&save_path)?;
        Ok(Benchmark { records, writer })
    }

    pub fn save(&mut self) -> eyre::Result<()> {
        for record in &self.records {
            self.writer.serialize(record)?;
        }

        self.writer.flush()?;

        Ok(())
    }
}

pub struct Run {
    args: CliArgs,
    cfg: PMMCfg,
}

impl Run {
    fn new(args: CliArgs, cfg: PMMCfg) -> Self {
        Self { args, cfg }
    }

    fn run(&self) -> eyre::Result<()> {
        match &self.args.command {
            Command::FetchAccounts { .. } => self.fetch_accounts(),
            Command::Benchmark { .. } => self.benchmark(),
            Command::Single { .. } | Command::Multi { .. } => self.simulate(),
        }
    }

    fn fetch_accounts(&self) -> eyre::Result<()> {
        let Command::FetchAccounts { http_url, accounts_path, pmms, .. } = &self.args.command else { unreachable!() };

        let rpc_client = RpcClient::new(http_url.expose_secret().to_string());
        let env = Environment::new("", accounts_path, None, self.cfg.clone(), None)?;

        let (slot, fetched) = Misc::fetch_pmm_accounts(pmms, &rpc_client, &self.cfg)?;
        for (dex, accounts) in fetched {
            for (pubkey, account) in accounts {
                env.save_account_to_disk(&dex, &pubkey, &account, slot)?;
                info!("saved account {pubkey} for {dex}");
            }
        }

        info!("done fetching accounts at slot {slot}");
        Ok(())
    }

    fn benchmark(&self) -> eyre::Result<()> {
        let Command::Benchmark { common, datasets_path, pmms, steps } = &self.args.command else { unreachable!() };

        let rpc_client = RpcClient::new(common.http_url.expose_secret().to_string());
        let (src_mint, src_dec, src_name) = (common.src_token.get_addr(), common.src_token.get_decimals(), common.src_token.to_string());
        let (dst_mint, dst_dec, dst_name) = (common.dst_token.get_addr(), common.dst_token.get_decimals(), common.dst_token.to_string());
        let mints = vec![(src_mint, src_dec), (dst_mint, dst_dec)];

        let steps = [
            (steps[0] * 10f64.powi(src_dec as i32)) as u64,
            (steps[1] * 10f64.powi(src_dec as i32)) as u64,
            (steps[2] * 10f64.powi(src_dec as i32)) as u64,
        ];
        let steps_cnt = ((steps[1] - steps[0]) / steps[2] + 1) as u64;

        let time = Local::now().format("%Y%m%d-%H%M%S").to_string();
        let multi = MultiProgress::new();

        let (slot, accs_map) = if common.jit_accounts {
            let (s, m) = Misc::fetch_pmm_accounts(pmms, &rpc_client, &self.cfg)?;
            (Some(s), m)
        } else {
            Misc::read_accounts_disk(pmms, &common.accounts_path)?
        };

        thread::scope(|s| {
            let handles: Vec<_> = pmms
                .iter()
                .map(|pmm| {
                    let (cfg, multi, mints, time) = (&self.cfg, &multi, &mints, &time);
                    let (src_name, dst_name) = (&src_name, &dst_name);
                    let pmm_accounts = accs_map.get(pmm).cloned().unwrap_or_default();

                    s.spawn(move || -> eyre::Result<()> {
                        // start up the progress bar only when all the spawned threads
                        // have finished bootstrapping so there's no CLI progress bar race cond
                        let (mut env, src_ata, dst_ata) = multi.suspend(|| -> eyre::Result<_> {
                            let mut env = Environment::new(&common.programs_path, &common.accounts_path, Some(mints), cfg.clone(), slot)?;
                            env.load_programs(&[*pmm])?;
                            env.load_accounts(&pmm_accounts.clone())?;
                            env.setup_wallet(&src_mint, steps[1], consts::AIRDROP_AMOUNT)?;

                            let (src_ata, dst_ata) = (env.wallet_ata(&src_mint), env.wallet_ata(&dst_mint));

                            Ok((env, src_ata, dst_ata))
                        })?;

                        let market = cfg.get_market(pmm).unwrap_or_else(|| panic!("{} not configured", pmm)).to_string();

                        let pb = multi.add(ProgressBar::new(steps_cnt));
                        pb.set_style(
                            ProgressStyle::default_bar().template(consts::PROGRESS_TEMPLATE)?.progress_chars(consts::PROGRESS_CHARS),
                        );
                        pb.set_prefix(format!("{}", pmm));

                        let (mut records, mut warn_cnt) = (vec![], u64::default());
                        let routes: Vec<Vec<magnus_router_client::types::Route>> =
                            vec![vec![Route { dexes: vec![*pmm], weights: vec![100] }.into()]];
                        for amount_in in (steps[0]..=steps[1]).step_by(steps[2] as usize) {
                            env.reset_wallet(&src_mint, amount_in)?;
                            let order_id = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

                            let mut swap_builder = SwapBuilder::new();
                            let swap = swap_builder
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
                                .order_id(order_id);

                            let mut construct = ConstructSwap {
                                cfg: cfg.clone(),
                                builder: swap,
                                payer: env.wallet_pubkey(),
                                sta: src_ata,
                                dta: dst_ata,
                                src_mint,
                                dst_mint,
                            };

                            construct.attach_pmm_accs(pmm);
                            let swap_ix = construct.instruction();

                            let tx = Transaction::new_signed_with_payer(
                                &[swap_ix],
                                Some(&env.wallet_pubkey()),
                                &[&env.wallet],
                                env.latest_blockhash(),
                            );

                            let res = match env.send_transaction(tx) {
                                Ok(res) => res,
                                Err(e) => {
                                    if warn_cnt == 0 {
                                        pb.println(format!("[WARN] {}: {:?}", pmm, e));
                                    }
                                    warn_cnt += 1;
                                    pb.inc(1);
                                    continue;
                                }
                            };

                            let amount_out = env.get_event_amount_out(&res);

                            records.push(BenchmarkRecord {
                                slot: env.slot.unwrap_or_default(),
                                pmm: *pmm,
                                market: &market,
                                src_token: src_name,
                                dst_token: dst_name,
                                amount_in: amount_in as f64 / 10f64.powi(src_dec as i32),
                                amount_out: amount_out as f64 / 10f64.powi(dst_dec as i32),
                                compute_units: res.compute_units_consumed,
                            });

                            pb.set_message(format!("in: {:.2}", amount_in as f64 / 10f64.powi(src_dec as i32)));
                            pb.inc(1);
                        }

                        if warn_cnt > 0 {
                            pb.println(format!("[WARN] {}: {} total failures", pmm, warn_cnt));
                        }

                        let filename = format!("{}/{}_{}_{}_{}.csv", datasets_path, env.slot.unwrap_or_default(), pmm, market, time);
                        Benchmark::new(records.clone(), &filename)?.save().is_ok().then(|| {
                            pb.println(format!("[{}] saved {} records to {}", pmm, records.len(), filename));
                        });

                        Ok(())
                    })
                })
                .collect();

            for handle in handles {
                if let Err(e) = handle.join().expect("thread panicked") {
                    warn!(?e, "benchmark thread failed");
                }
            }
        });

        Ok(())
    }

    fn simulate(&self) -> eyre::Result<()> {
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
        for (d, w) in pmms.iter().zip(weights.iter()) {
            assert_eq!(d.len(), w.len(), "dexes and weights length mismatch");
        }

        let rpc_client = RpcClient::new(common.http_url.expose_secret().to_string());
        let flat_pmms: Vec<Dex> = pmms.iter().flatten().copied().collect();

        let (src_mint, src_dec, src_name) = (common.src_token.get_addr(), common.src_token.get_decimals(), common.src_token.to_string());
        let (dst_mint, dst_dec, dst_name) = (common.dst_token.get_addr(), common.dst_token.get_decimals(), common.dst_token.to_string());
        let mints = vec![(src_mint, src_dec), (dst_mint, dst_dec)];

        let mut env = Environment::new(&common.programs_path, &common.accounts_path, Some(&mints), self.cfg.clone(), None)?;
        env.load_programs(&flat_pmms)?;
        env.fetch_and_load_accounts(&flat_pmms, common.jit_accounts, Some(&rpc_client))?;

        let norm_amount_in: Vec<u64> = amount_in.iter().map(|amount| amount * 10f64.powi(src_dec as i32)).map(|a| a as u64).collect();
        let norm_amount_in_sum: u64 = norm_amount_in.iter().sum();

        // - mint only the source token's desired amount (i.e the amount we're going to swap)
        // - airdrop some SOL to cover fees
        env.setup_wallet(&src_mint, norm_amount_in_sum, consts::AIRDROP_AMOUNT)?;
        info!(?env);

        let (src_ata, dst_ata) = (env.wallet_ata(&src_mint), env.wallet_ata(&dst_mint));
        let (src_before, dst_before) = (
            env.token_balance(&src_mint) as f64 / 10_f64.powi(src_dec as i32),
            env.token_balance(&dst_mint) as f64 / 10_f64.powi(dst_dec as i32),
        );
        info!("before: {} = {:.6} | {} = {:.6} | ", src_name, src_before, dst_name, dst_before);

        let routes: Vec<Vec<magnus_router_client::types::Route>> = pmms
            .iter()
            .zip(weights.iter())
            .map(|(dex_grp, weight_group)| vec![Route { dexes: dex_grp.clone(), weights: weight_group.clone() }.into()])
            .collect();
        info!("swapping {:?} {} via routes: {:?}", norm_amount_in, src_name, routes);

        let order_id = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let mut swap_builder = SwapBuilder::new();
        let swap = swap_builder
            .payer(env.wallet_pubkey())
            .source_token_account(src_ata)
            .destination_token_account(dst_ata)
            .source_mint(src_mint)
            .destination_mint(dst_mint)
            .amount_in(norm_amount_in_sum)
            .expect_amount_out(1)
            .min_return(1)
            .amounts(norm_amount_in)
            .routes(routes)
            .order_id(order_id);

        let mut construct = ConstructSwap {
            cfg: self.cfg.clone(),
            builder: swap,
            payer: env.wallet_pubkey(),
            sta: src_ata,
            dta: dst_ata,
            src_mint,
            dst_mint,
        };

        // attach the necessary accounts for each of the implemented Prop AMMs
        flat_pmms.iter().for_each(|pmm| {
            construct.attach_pmm_accs(pmm);
        });

        let swap_ix = construct.instruction();
        debug!("router program id: {}", swap_ix.program_id);

        let tx = Transaction::new_signed_with_payer(&[swap_ix], Some(&env.wallet_pubkey()), &[&env.wallet], env.latest_blockhash());
        let res = env.send_transaction(tx).expect("failed to exec tx");
        let amount_out = env.get_event_amount_out(&res);

        let (src_after, dst_after) = (
            env.token_balance(&src_mint) as f64 / 10_f64.powi(src_dec as i32),
            env.token_balance(&dst_mint) as f64 / 10_f64.powi(dst_dec as i32),
        );

        info!("|SWAP EXECUTED| compute units consumed: {:?} | amount_out: {}", res.compute_units_consumed, amount_out);
        info!("after: {} = {:.6} | {} = {:.6} | ", src_name, src_after, dst_name, dst_after);
        info!("diff: {} spent = {:.6} | {} received = {:.6}", src_name, src_before - src_after, dst_name, dst_after - dst_before);

        Ok(())
    }
}

fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_thread_ids(true)
        .with_line_number(true)
        .with_target(true)
        .with_timer(UtcTime::rfc_3339())
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::default().add_directive(tracing::Level::INFO.into())),
        )
        .init();

    let args = CliArgs::parse();
    info!(?args, command = args.command.name());

    let cfg = PMMCfg::load(args.command.setup_path())?;

    Run::new(args, cfg).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nested_pmms_json_single() {
        let input = r#"[["humidifi"]]"#;
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi]]);
    }

    #[test]
    fn test_parse_nested_pmms_json_multiple() {
        let input = r#"[["humidifi","obric-v2"],["zerofi"]]"#;
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2], vec![Dex::Zerofi]]);
    }

    #[test]
    fn test_parse_nested_pmms_no_quotes_single() {
        let input = "[[humidifi]]";
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi]]);
    }

    #[test]
    fn test_parse_nested_pmms_no_quotes_single_route_multiple_pmms() {
        let input = "[[humidifi,obric-v2]]";
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2]]);
    }

    #[test]
    fn test_parse_nested_pmms_no_quotes_multiple_routes() {
        let input = "[[humidifi,obric-v2],[zerofi]]";
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2], vec![Dex::Zerofi]]);
    }

    #[test]
    fn test_parse_nested_pmms_no_quotes_three_routes() {
        let input = "[[humidifi],[obric-v2,solfi-v2],[zerofi]]";
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi], vec![Dex::ObricV2, Dex::SolfiV2], vec![Dex::Zerofi],]);
    }

    #[test]
    fn test_parse_nested_pmms_no_quotes_all_pmms() {
        let input = "[[raydium-cl-v2,raydium-cp],[obric-v2,solfi-v2,zerofi,humidifi]]";
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::RaydiumClV2, Dex::RaydiumCp], vec![Dex::ObricV2, Dex::SolfiV2, Dex::Zerofi, Dex::Humidifi],]);
    }

    #[test]
    fn test_parse_nested_pmms_no_quotes_with_spaces() {
        let input = "[[ humidifi , obric-v2 ],[ zerofi ]]";
        let result = CliArgs::parse_nested_pmms(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2], vec![Dex::Zerofi]]);
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
        for (d, w) in pmms.iter().zip(weights.iter()) {
            assert_eq!(d.len(), w.len());
        }
    }

    #[test]
    fn test_parse_step_valid() {
        let result = CliArgs::parse_steps("1.0,100.0,0.5").unwrap();
        assert_eq!(result, [1.0, 100.0, 0.5]);
    }

    #[test]
    fn test_parse_step_with_spaces() {
        let result = CliArgs::parse_steps("1.0, 100.0, 0.5").unwrap();
        assert_eq!(result, [1.0, 100.0, 0.5]);
    }

    #[test]
    fn test_parse_step_start_gte_end() {
        let result = CliArgs::parse_steps("100.0,50.0,1.0");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("start must be less than end"));
    }

    #[test]
    fn test_parse_step_negative_step() {
        let result = CliArgs::parse_steps("1.0,100.0,-1.0");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("step must be positive"));
    }
}
