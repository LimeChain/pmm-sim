#![doc = include_str!("../README.md")]
#![allow(clippy::type_complexity, clippy::result_large_err)]
#![deny(unused)]
//! .
//! .
use std::{
    collections::HashSet,
    fmt::Display,
    fs::{self, File},
    io::Write,
    path::Path,
    str::FromStr,
    time::SystemTime,
};

use base64::{Engine, engine::general_purpose};
use clap::{Args, Parser, Subcommand};
use litesvm::LiteSVM;
use magnus_router_client::instructions::SwapBuilder;
use magnus_shared::{Dex, Route};
use secrecy::{ExposeSecret, SecretString};
use solana_client::rpc_client::RpcClient;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_sdk::{
    account::Account, message::AccountMeta, program_pack::Pack, pubkey::Pubkey, rent::Rent, signature::Keypair,
    signer::Signer, sysvar, transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use tracing::{debug, info};
use tracing_subscriber::{EnvFilter, fmt::time::UtcTime};

pub mod consts {
    use solana_sdk::{pubkey, pubkey::Pubkey};

    pub const PROGRAMS_DIR: &str = "cfg/programs";
    pub const ACCOUNTS_DIR: &str = "cfg/accounts";

    pub const WSOL: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
    pub const WSOL_DECIMALS: u8 = 9;

    pub const USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    pub const USDC_DECIMALS: u8 = 6;

    pub const USDT: Pubkey = pubkey!("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB");
    pub const USDT_DECIMALS: u8 = 6;

    pub mod solfi_v2 {
        use super::*;

        pub const MARKET: Pubkey = pubkey!("65ZHSArs5XxPseKQbB1B4r16vDxMWnCxHMzogDAqiDUc");
        pub const POOL_BASE_VAULT: Pubkey = pubkey!("CRo8DBwrmd97DJfAnvCv96tZPL5Mktf2NZy2ZnhDer1A");
        pub const POOL_QUOTE_VAULT: Pubkey = pubkey!("GhFfLFSprPpfoRaWakPMmJTMJBHuz6C694jYwxy2dAic");
        pub const CFG: Pubkey = pubkey!("FmxXDSR9WvpJTCh738D1LEDuhMoA8geCtZgHb3isy7Dp");
        pub const ORACLE: Pubkey = pubkey!("2ny7eGyZCoeEVTkNLf5HcnJFBKkyA4p4gcrtb3b8y8ou");

        pub const ACCOUNTS: [Pubkey; 5] = [MARKET, POOL_BASE_VAULT, POOL_QUOTE_VAULT, CFG, ORACLE];
    }

    pub mod humidifi {
        use super::*;

        pub const MARKET: Pubkey = pubkey!("DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW");
        pub const BASE_TOKEN_ACCOUNT: Pubkey = pubkey!("8BrVfsvzb1DZqCactbYWoKSv24AfsLBuXJqzpzYCwznF");
        pub const QUOTE_TOKEN_ACCOUNT: Pubkey = pubkey!("HsQcHFFNUVTp3MWrXYbuZchBNd4Pwk8636bKzLvpfYNR");

        pub const ACCOUNTS: [Pubkey; 3] = [MARKET, BASE_TOKEN_ACCOUNT, QUOTE_TOKEN_ACCOUNT];
    }

    pub mod zerofi {
        use super::*;

        pub const PAIR: Pubkey = pubkey!("2h9hhu3gxY9kCdXEwdTHV8yPAMYVoHgKopRyG1HbDwfi");
        pub const VAULT_INFO_BASE: Pubkey = pubkey!("7RHJ2WfexqUxy7SXfbNZRZDgZi3D9jtMAQp9VhfzpU8T");
        pub const VAULT_BASE: Pubkey = pubkey!("ERP5RTV6cWmoGrv7r9W2V5pbgDFSepc4j97qNnx1Jris");
        pub const VAULT_INFO_QUOTE: Pubkey = pubkey!("Ef7zPqj4NuZHwaTczUTY9oRbxXrfZseUcKcqPaidCZ5W");
        pub const VAULT_QUOTE: Pubkey = pubkey!("7wYJVD8iXmMQjND1fwi1hPr68QwruVVtirbotyJZXaVH");

        pub const ACCOUNTS: [Pubkey; 5] = [PAIR, VAULT_INFO_BASE, VAULT_BASE, VAULT_INFO_QUOTE, VAULT_QUOTE];
    }

    pub mod obric_v2 {
        use super::*;

        pub const TRADING_PAIR: Pubkey = pubkey!("BWBHrYqfcjAh5dSiRwzPnY4656cApXVXmkeDmAfwBKQG");
        pub const SECOND_REF_ORACLE: Pubkey = pubkey!("GZsNmWKbqhMYtdSkkvMdEyQF9k5mLmP7tTKYWZjcHVPE");
        pub const THIRD_REF_ORACLE: Pubkey = pubkey!("6YawcNeZ74tRyCv4UfGydYMr7eho7vbUR6ScVffxKAb3");
        pub const RESERVE_X: Pubkey = pubkey!("C3tPQ8TRcHybnPpR8KMASUVD3PukQRRHEsLwxorJMhgm");
        pub const RESERVE_Y: Pubkey = pubkey!("AAamGhyPfpQJWfZHTq944NM1cFvoVLDrQxt7HGjeRQUS");
        pub const REF_ORACLE: Pubkey = pubkey!("J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw");
        pub const X_PRICE_FEED: Pubkey = pubkey!("J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw");
        pub const Y_PRICE_FEED: Pubkey = pubkey!("J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw");

        pub const ACCOUNTS: [Pubkey; 8] = [
            TRADING_PAIR,
            SECOND_REF_ORACLE,
            THIRD_REF_ORACLE,
            RESERVE_X,
            RESERVE_Y,
            REF_ORACLE,
            X_PRICE_FEED,
            Y_PRICE_FEED,
        ];
    }
}

#[derive(Parser, Debug)]
#[command(version, about = "Simulate Solana Prop AMM swaps locally", long_about = None)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Command,
}

impl CliArgs {
    fn parse_nested_dexes(s: &str) -> Result<Vec<Vec<Dex>>, String> {
        if let Ok(parsed) = serde_json::from_str::<Vec<Vec<String>>>(s) {
            return parsed
                .into_iter()
                .map(|group| group.into_iter().map(|s| s.parse::<Dex>()).collect::<Result<Vec<Dex>, _>>())
                .collect();
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

    fn default_pmm_dexes() -> Vec<Dex> {
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

    #[arg(long, env = "AMOUNT_IN", default_value_t = 1, help = "The amount of tokens to trade")]
    pub amount_in: u64,

    #[arg(
        long,
        env = "DIRECTION",
        default_value = "wsol-to-usdc",
        help = "Swap direction: wsol-to-usdc or usdc-to-wsol"
    )]
    pub direction: Direction,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(
        about = "Run a single swap instruction across one or more Prop AMMs with specified weights",
        after_help = "Examples:
  pmm-sim single --dexes humidifi --weights 100
  pmm-sim single --dexes humidifi,obric-v2 --weights 50,50
  pmm-sim single --amount-in 1000000 --dexes zerofi,solfi-v2 --weights 60,40"
    )]
    Single {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(
            long,
            value_delimiter = ',',
            default_value = "humidifi,obric-v2",
            help = "Comma-separated list of Prop AMMs"
        )]
        dexes: Vec<Dex>,

        #[arg(long, value_delimiter = ',', default_value = "50,50", help = "Comma-separated weights")]
        weights: Vec<u8>,
    },

    #[command(
        about = "Execute multiple swap instructions across nested Prop AMM routes. Each inner list represents a \
                 single transaction step.",
        after_help = "Examples:
      # Single step with one DEX
      pmm-sim multi --dexes '[[humidifi]]' --weights '[[100]]'

      # Two sequential swaps: (Humidifi + Obric) followed by Zerofi
      pmm-sim multi --dexes '[[humidifi,obric-v2],[zerofi]]' --weights '[[50,50],[100]]'

      # Complex three-step route
      pmm-sim multi --amount-in 10 --dexes '[[humidifi],[obric-v2,solfi-v2],[zerofi]]' --weights \
                      '[[100],[60,40],[100]]'"
    )]
    Multi {
        #[command(flatten)]
        common: CommonArgs,

        #[arg(long, default_value = "[[humidifi]]", help = "JSON nested routes, e.g. '[[dex1,dex2],[dex3]]'")]
        dexes: String,

        #[arg(
            long,
            default_value = "[[100]]",
            help = "JSON nested weights matching the dexes structure, e.g. '[[50,50],[100]]'"
        )]
        weights: String,
    },

    #[command(
        about = "Fetch accounts from the specified Pmms via RPC and save them locally (presumably for later usage)",
        after_help = "Examples:
  pmm-sim fetch-accounts --dexes humidifi
  pmm-sim fetch-accounts --dexes humidifi,obric-v2,zerofi,solfi-v2pmm-sim \
                      fetch-accounts --dexes humidifi --http-url https://my-rpc.com"
    )]
    FetchAccounts {
        #[arg(long, env = "HTTP_URL", default_value = "https://api.mainnet.solana.com")]
        http_url: SecretString,

        #[arg(
            long,
            value_delimiter = ',',
            default_values_t = CliArgs::default_pmm_dexes(),
            help = "Comma-separated list of dexes to fetch accounts for"
        )]
        dexes: Vec<Dex>,

        #[arg(long, default_value = consts::ACCOUNTS_DIR, help = "Directory to save fetched accounts")]
        output_dir: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    WsolToUsdc,
    UsdcToWsol,
}

impl FromStr for Direction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wsol-to-usdc" | "wsoltousdc" | "wsol>usdc" | "wsol" => Ok(Direction::WsolToUsdc),
            "usdc-to-wsol" | "usdctowsol" | "usdc>wsol" | "usdc" => Ok(Direction::UsdcToWsol),
            _ => Err(format!("invalid direction '{}'. valid options: wsol-to-usdc, usdc-to-wsol", s)),
        }
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::WsolToUsdc => f.write_str("wsol-to-usdc"),
            Direction::UsdcToWsol => f.write_str("usdc-to-wsol"),
        }
    }
}

struct Environment<'a, P: Into<String> + Display + Clone> {
    svm: LiteSVM,
    wallet: Keypair,
    mints: Option<&'a [(Pubkey, u8)]>,

    programs_path: P,
    accounts_path: P,
}

impl<'a, P: Into<String> + Display + Clone> Environment<'a, P> {
    fn new(programs_path: P, accounts_path: P, mints: Option<&[(Pubkey, u8)]>) -> eyre::Result<Environment<'_, P>> {
        let mut budget = ComputeBudget::new_with_defaults(false);
        budget.compute_unit_limit = 2_000_000;

        let wallet = Keypair::new();
        let mut svm =
            LiteSVM::new().with_default_programs().with_sysvars().with_sigverify(true).with_compute_budget(budget);

        if let Some(mints) = mints {
            for (mint, mint_decimals) in mints {
                svm.set_account(*mint, Misc::mk_mint_acc(*mint_decimals))?;
            }
        }

        Ok(Environment { svm, wallet, programs_path, accounts_path, mints })
    }

    fn setup_wallet(&mut self, mint: &Pubkey, mint_amount: u64, airdrop_amount: u64) -> eyre::Result<()> {
        // create the ATAs for all initialised mints
        if let Some(mints) = self.mints {
            for (mint, _) in mints {
                let ata = get_associated_token_address(&self.wallet.pubkey(), mint);
                self.svm.set_account(ata, Misc::mk_ata(mint, &self.wallet.pubkey(), 0))?;
            }
        }

        let ata = get_associated_token_address(&self.wallet.pubkey(), mint);
        self.svm.set_account(ata, Misc::mk_ata(mint, &self.wallet.pubkey(), mint_amount))?;

        self.svm.airdrop(&self.wallet.pubkey(), airdrop_amount).expect("airdrop failed");

        Ok(())
    }

    fn get_decimals(&self, mint: &Pubkey) -> u8 {
        match *mint {
            consts::WSOL => consts::WSOL_DECIMALS,
            consts::USDC => consts::USDC_DECIMALS,
            consts::USDT => consts::USDT_DECIMALS,
            _ => 9,
        }
    }

    fn get_mint_naming(&self, mint: &Pubkey) -> &'static str {
        match *mint {
            consts::WSOL => "WSOL",
            consts::USDC => "USDC",
            consts::USDT => "USDT",
            _ => "UNKNOWN",
        }
    }

    fn wallet_pubkey(&self) -> Pubkey {
        self.wallet.pubkey()
    }

    fn wallet_ata(&self, mint: &Pubkey) -> Pubkey {
        get_associated_token_address(&self.wallet.pubkey(), mint)
    }

    fn load_programs(&mut self, dexes: &[Dex]) -> eyre::Result<()> {
        // mandatory load
        self.svm.add_program_from_file(
            magnus_router_client::programs::ROUTER_ID,
            format!("{}/magnus-router.so", self.programs_path),
        )?;

        let unique_dexes: HashSet<_> = dexes.iter().collect();
        for dex in unique_dexes {
            let program_id = dex.program_id();

            self.svm.add_program_from_file(
                Pubkey::new_from_array(program_id.to_bytes()),
                format!("{}/{}.so", self.programs_path, dex),
            )?;

            info!("loaded program for {dex}");
        }

        Ok(())
    }

    fn load_accounts(&mut self, dexes: &[Dex], jit: bool, client: Option<&RpcClient>) -> eyre::Result<()> {
        match jit {
            true => {
                let rpc_client = client.expect("RPC client is required for JIT account loading");
                self.load_jit_accounts(dexes, rpc_client)?;
            }
            false => {
                self.load_static_accounts(dexes)?;
            }
        }

        Ok(())
    }

    fn load_static_accounts(&mut self, dexes: &[Dex]) -> eyre::Result<()> {
        let unique_dexes: HashSet<_> = dexes.iter().collect();
        let mut all_slots: Vec<u64> = vec![];

        for dex in unique_dexes {
            let (accounts, slot) = self.read_accounts_from_disk(dex)?;

            for (pubkey, account) in accounts {
                self.svm.set_account(pubkey, account)?;
                debug!("loaded account {pubkey} for {dex}");
            }

            if let Some(s) = slot {
                all_slots.push(s);
            }

            info!("loaded accounts for {dex}");
        }

        // check if for some reason the slots logged in the accs config files differ
        // if they do, warp up to the first slot
        if !all_slots.is_empty() {
            let first_slot = all_slots[0];
            if all_slots.iter().any(|&s| s != first_slot) {
                let min_slot = all_slots.iter().min().copied().unwrap();
                let max_slot = all_slots.iter().max().copied().unwrap();
                tracing::warn!(
                    "slot mismatch across dexes: accounts fetched at different slots ({min_slot} - {max_slot}), using \
                     {first_slot}"
                );
            }
            self.svm.warp_to_slot(first_slot);
            info!("warped to slot {first_slot}");
        }

        Ok(())
    }

    fn load_jit_accounts(&mut self, dexes: &[Dex], client: &RpcClient) -> eyre::Result<()> {
        let (slot, fetched) = Misc::fetch_dex_accounts(dexes, client)?;

        for (dex, accounts) in fetched {
            for (pubkey, account) in accounts {
                self.svm.set_account(pubkey, account)?;
                debug!("loaded account {pubkey} for {dex}");
            }
            info!("loaded accounts for {dex}");
        }

        self.svm.warp_to_slot(slot);
        info!("warped to slot {slot}");

        Ok(())
    }

    fn read_accounts_from_disk(&self, dex: &Dex) -> eyre::Result<(Vec<(Pubkey, Account)>, Option<u64>)> {
        let prefix = dex.to_string();
        let accounts_path = format!("{}", self.accounts_path);
        let data_dir = Path::new(&accounts_path);

        if !data_dir.exists() {
            return Ok((vec![], None));
        }

        let mut accounts = vec![];
        let mut slots: Vec<u64> = vec![];

        for entry in fs::read_dir(data_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json"))
            {
                let (pubkey, account, slot) = Self::read_account_from_file(&path)?;
                accounts.push((pubkey, account));
                if let Some(s) = slot {
                    slots.push(s);
                }
            }
        }

        let slot = if slots.is_empty() {
            None
        } else {
            let first_slot = slots[0];
            if slots.iter().any(|&s| s != first_slot) {
                let min_slot = slots.iter().min().copied().unwrap();
                let max_slot = slots.iter().max().copied().unwrap();
                tracing::warn!(
                    "slot mismatch for {dex}: accounts fetched at different slots ({min_slot} - {max_slot}), using \
                     {first_slot}"
                );
            }
            Some(first_slot)
        };

        Ok((accounts, slot))
    }

    fn read_account_from_file(path: &Path) -> eyre::Result<(Pubkey, Account, Option<u64>)> {
        let contents = fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&contents)?;

        let pubkey = Pubkey::from_str(value["pubkey"].as_str().ok_or_else(|| eyre::eyre!("missing pubkey"))?)?;
        let lamports = value["account"]["lamports"].as_u64().ok_or_else(|| eyre::eyre!("missing lamports"))?;
        let data_base64 = value["account"]["data"][0].as_str().ok_or_else(|| eyre::eyre!("missing data"))?;
        let data = general_purpose::STANDARD.decode(data_base64)?;
        let owner = Pubkey::from_str(value["account"]["owner"].as_str().ok_or_else(|| eyre::eyre!("missing owner"))?)?;
        let executable = value["account"]["executable"].as_bool().ok_or_else(|| eyre::eyre!("missing executable"))?;
        let rent_epoch = value["account"]["rentEpoch"].as_u64().ok_or_else(|| eyre::eyre!("missing rentEpoch"))?;
        let slot = value["slot"].as_u64(); // Optional, for backwards compatibility

        Ok((pubkey, Account { lamports, data, owner, executable, rent_epoch }, slot))
    }

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
}

pub struct ConstructSwap<'a> {
    builder: &'a mut SwapBuilder,
    payer: Pubkey,
    sta: Pubkey,
    dta: Pubkey,
}

impl<'a> ConstructSwap<'a> {
    fn instruction(&self) -> solana_sdk::instruction::Instruction {
        self.builder.instruction()
    }

    pub fn attach_solfiv2_accs(&mut self) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(
                Pubkey::from_str(&magnus_shared::pmm_solfi_v2::id().to_string()).unwrap(),
                false,
            ))
            .add_remaining_account(AccountMeta::new(self.payer, true))
            .add_remaining_account(AccountMeta::new(self.sta, false))
            .add_remaining_account(AccountMeta::new(self.dta, false))
            .add_remaining_account(AccountMeta::new(consts::solfi_v2::MARKET, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::solfi_v2::ORACLE, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::solfi_v2::CFG, false))
            .add_remaining_account(AccountMeta::new(consts::solfi_v2::POOL_BASE_VAULT, false))
            .add_remaining_account(AccountMeta::new(consts::solfi_v2::POOL_QUOTE_VAULT, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::WSOL, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::USDC, false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::instructions::id(), false));
    }

    pub fn attach_humidifi_accs(&mut self) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(
                Pubkey::from_str(&magnus_shared::pmm_humidifi::id().to_string()).unwrap(),
                false,
            ))
            .add_remaining_account(AccountMeta::new(self.payer, true))
            .add_remaining_account(AccountMeta::new(self.sta, false))
            .add_remaining_account(AccountMeta::new(self.dta, false))
            .add_remaining_account(AccountMeta::new_readonly(Misc::create_humidifi_param(1500), false))
            .add_remaining_account(AccountMeta::new(consts::humidifi::MARKET, false))
            .add_remaining_account(AccountMeta::new(consts::humidifi::BASE_TOKEN_ACCOUNT, false))
            .add_remaining_account(AccountMeta::new(consts::humidifi::QUOTE_TOKEN_ACCOUNT, false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::clock::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::instructions::id(), false));
    }

    pub fn attach_zerofi_accs(&mut self) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(
                Pubkey::from_str(&magnus_shared::pmm_zerofi::id().to_string()).unwrap(),
                false,
            ))
            .add_remaining_account(AccountMeta::new(self.payer, true))
            .add_remaining_account(AccountMeta::new(self.sta, false))
            .add_remaining_account(AccountMeta::new(self.dta, false))
            .add_remaining_account(AccountMeta::new(consts::zerofi::PAIR, false))
            .add_remaining_account(AccountMeta::new(consts::zerofi::VAULT_INFO_BASE, false))
            .add_remaining_account(AccountMeta::new(consts::zerofi::VAULT_BASE, false))
            .add_remaining_account(AccountMeta::new(consts::zerofi::VAULT_INFO_QUOTE, false))
            .add_remaining_account(AccountMeta::new(consts::zerofi::VAULT_QUOTE, false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::instructions::id(), false));
    }

    pub fn attach_obric_v2_accs(&mut self) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(
                Pubkey::from_str(&magnus_shared::pmm_obric_v2::id().to_string()).unwrap(),
                false,
            ))
            .add_remaining_account(AccountMeta::new(self.payer, true))
            .add_remaining_account(AccountMeta::new(self.sta, false))
            .add_remaining_account(AccountMeta::new(self.dta, false))
            .add_remaining_account(AccountMeta::new(consts::obric_v2::TRADING_PAIR, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::obric_v2::SECOND_REF_ORACLE, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::obric_v2::THIRD_REF_ORACLE, false))
            .add_remaining_account(AccountMeta::new(consts::obric_v2::RESERVE_X, false))
            .add_remaining_account(AccountMeta::new(consts::obric_v2::RESERVE_Y, false))
            .add_remaining_account(AccountMeta::new(consts::obric_v2::REF_ORACLE, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::obric_v2::X_PRICE_FEED, false))
            .add_remaining_account(AccountMeta::new_readonly(consts::obric_v2::Y_PRICE_FEED, false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false));
    }
}

struct Misc;
impl Misc {
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

    fn create_humidifi_param(swap_id: u64) -> Pubkey {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&swap_id.to_le_bytes());
        Pubkey::new_from_array(bytes)
    }

    fn get_dex_accounts(dex: &Dex) -> Option<Vec<Pubkey>> {
        match dex {
            Dex::Humidifi => Some(consts::humidifi::ACCOUNTS.to_vec()),
            Dex::SolfiV2 => Some(consts::solfi_v2::ACCOUNTS.to_vec()),
            Dex::Zerofi => Some(consts::zerofi::ACCOUNTS.to_vec()),
            Dex::ObricV2 => Some(consts::obric_v2::ACCOUNTS.to_vec()),
            _ => None,
        }
    }

    fn fetch_dex_accounts(
        dexes: &[Dex],
        client: &RpcClient,
    ) -> eyre::Result<(u64, Vec<(Dex, Vec<(Pubkey, Account)>)>)> {
        let slot = client.get_slot()?;
        let unique_dexes: HashSet<_> = dexes.iter().collect();
        let mut results = vec![];

        for dex in unique_dexes {
            let Some(accounts) = Misc::get_dex_accounts(dex) else {
                tracing::warn!("skipping unsupported dex: {dex}");
                continue;
            };

            info!("fetching {} accounts for {dex} at slot {slot}", accounts.len());

            let fetched = client.get_multiple_accounts(&accounts)?;
            let mut dex_accounts = vec![];
            for (pubkey, account) in accounts.iter().zip(fetched.into_iter()) {
                if let Some(acc) = account {
                    dex_accounts.push((*pubkey, acc));
                } else {
                    tracing::warn!("account {pubkey} not found for {dex}");
                }
            }

            results.push((*dex, dex_accounts));
        }

        Ok((slot, results))
    }
}

pub struct Run {
    args: CliArgs,
}

impl Run {
    fn new(args: CliArgs) -> Self {
        Self { args }
    }

    fn run(&self) -> eyre::Result<()> {
        match &self.args.command {
            Command::FetchAccounts { .. } => self.fetch_accounts(),
            Command::Single { .. } | Command::Multi { .. } => self.simulate(),
        }
    }

    fn fetch_accounts(&self) -> eyre::Result<()> {
        let Command::FetchAccounts { http_url, dexes, output_dir } = &self.args.command else { unreachable!() };

        let rpc_client = RpcClient::new(http_url.expose_secret().to_string());
        let env = Environment::new(consts::PROGRAMS_DIR, output_dir.as_str(), None)?;

        let (slot, fetched) = Misc::fetch_dex_accounts(dexes, &rpc_client)?;
        for (dex, accounts) in fetched {
            for (pubkey, account) in accounts {
                env.save_account_to_disk(&dex, &pubkey, &account, slot)?;
                info!("saved account {pubkey} for {dex}");
            }
        }

        info!("done fetching accounts at slot {slot}");
        Ok(())
    }

    fn simulate(&self) -> eyre::Result<()> {
        let (common, dexes, weights) = match &self.args.command {
            Command::Single { common, dexes, weights } => (common, vec![dexes.clone()], vec![weights.clone()]),
            Command::Multi { common, dexes, weights } => {
                let pmms = CliArgs::parse_nested_dexes(dexes).expect("invalid format for nested dexes");
                let weights = CliArgs::parse_nested_weights(weights).expect("invalid format for nested weights");

                (common, pmms, weights)
            }
            _ => unreachable!(),
        };

        // ensure that each dex has a corresponding weight
        assert_eq!(dexes.len(), weights.len(), "dexes and weights outer length mismatch");
        for (d, w) in dexes.iter().zip(weights.iter()) {
            assert_eq!(d.len(), w.len(), "dexes and weights length mismatch");
        }

        let rpc_client = RpcClient::new(common.http_url.expose_secret().to_string());
        let flat_dexes: Vec<Dex> = dexes.iter().flatten().copied().collect();

        let mints = vec![(consts::WSOL, consts::WSOL_DECIMALS), (consts::USDC, consts::USDC_DECIMALS)];
        let mut env = Environment::new(consts::PROGRAMS_DIR, consts::ACCOUNTS_DIR, Some(&mints))?;
        env.load_programs(&flat_dexes)?;
        env.load_accounts(&flat_dexes, common.jit_accounts, Some(&rpc_client))?;

        let (src_mint, dst_mint) = match common.direction {
            Direction::WsolToUsdc => (consts::WSOL, consts::USDC),
            Direction::UsdcToWsol => (consts::USDC, consts::WSOL),
        };

        // - mint only the source token's desired amount (i.e the amount we're going to swap)
        // - airdrop some SOL to cover fees
        env.setup_wallet(&src_mint, common.amount_in * 10u64.pow(env.get_decimals(&src_mint) as u32), 10_000_000_000)?;

        let (src_ata, src_name) = (env.wallet_ata(&src_mint), env.get_mint_naming(&src_mint));
        let (dst_ata, dst_name) = (env.wallet_ata(&dst_mint), env.get_mint_naming(&dst_mint));

        let src_before = env.token_balance(&src_mint) as f64 / 10_f64.powi(env.get_decimals(&src_mint) as i32);
        let dst_before = env.token_balance(&dst_mint) as f64 / 10_f64.powi(env.get_decimals(&dst_mint) as i32);
        info!("before: {} ({}) = {} | {} ({}) = {}", src_name, src_mint, src_before, dst_name, dst_mint, dst_before);

        let routes: Vec<Route> = dexes
            .iter()
            .zip(weights.iter())
            .map(|(dex, weight)| Route { dexes: dex.clone(), weights: weight.clone() })
            .collect();

        let norm_amount_in = common.amount_in * 10u64.pow(env.get_decimals(&src_mint) as u32);
        info!("swapping {} {} via routes: {:?}", common.amount_in, src_name, routes);

        let mut swap_builder = SwapBuilder::new();
        let swap = swap_builder
            .payer(env.wallet_pubkey())
            .source_token_account(src_ata)
            .destination_token_account(dst_ata)
            .source_mint(src_mint)
            .destination_mint(dst_mint)
            .amount_in(norm_amount_in)
            .expect_amount_out(1)
            .min_return(1)
            .amounts(vec![norm_amount_in])
            .routes(vec![routes.iter().map(|route| route.clone().into()).collect()])
            .order_id(SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());

        let mut construct = ConstructSwap { builder: swap, payer: env.wallet_pubkey(), sta: src_ata, dta: dst_ata };

        // attach the necessary accounts for each of the implemented Prop AMMs
        for dex in flat_dexes.iter() {
            match dex {
                Dex::Humidifi => construct.attach_humidifi_accs(),
                Dex::SolfiV2 => construct.attach_solfiv2_accs(),
                Dex::Zerofi => construct.attach_zerofi_accs(),
                Dex::ObricV2 => construct.attach_obric_v2_accs(),
                _ => {}
            };
        }

        let swap_ix = construct.instruction();
        debug!("router program id: {}", swap_ix.program_id);

        let tx = Transaction::new_signed_with_payer(
            &[swap_ix],
            Some(&env.wallet_pubkey()),
            &[&env.wallet],
            env.latest_blockhash(),
        );
        env.send_transaction(tx).expect("failed to exec tx");

        let (src_after, dst_after) = (
            env.token_balance(&src_mint) as f64 / 10_f64.powi(env.get_decimals(&src_mint) as i32),
            env.token_balance(&dst_mint) as f64 / 10_f64.powi(env.get_decimals(&dst_mint) as i32),
        );
        info!("after: {} = {:.6} | {} = {:.6}", src_name, src_after, dst_name, dst_after);
        info!(
            "diff: {} spent = {:.6} | {} received = {:.6}",
            src_name,
            src_before - src_after,
            dst_name,
            dst_after - dst_before
        );

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
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::default().add_directive(tracing::Level::INFO.into())),
        )
        .init();

    let args = CliArgs::parse();
    info!(?args);

    Run::new(args).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nested_dexes_json_single() {
        let input = r#"[["humidifi"]]"#;
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi]]);
    }

    #[test]
    fn test_parse_nested_dexes_json_multiple() {
        let input = r#"[["humidifi","obric-v2"],["zerofi"]]"#;
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2], vec![Dex::Zerofi]]);
    }

    #[test]
    fn test_parse_nested_dexes_no_quotes_single() {
        let input = "[[humidifi]]";
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi]]);
    }

    #[test]
    fn test_parse_nested_dexes_no_quotes_single_route_multiple_dexes() {
        let input = "[[humidifi,obric-v2]]";
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2]]);
    }

    #[test]
    fn test_parse_nested_dexes_no_quotes_multiple_routes() {
        let input = "[[humidifi,obric-v2],[zerofi]]";
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2], vec![Dex::Zerofi]]);
    }

    #[test]
    fn test_parse_nested_dexes_no_quotes_three_routes() {
        let input = "[[humidifi],[obric-v2,solfi-v2],[zerofi]]";
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi], vec![Dex::ObricV2, Dex::SolfiV2], vec![Dex::Zerofi],]);
    }

    #[test]
    fn test_parse_nested_dexes_no_quotes_all_dexes() {
        let input = "[[raydium-cl-v2,raydium-cp],[obric-v2,solfi-v2,zerofi,humidifi]]";
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(
            result,
            vec![vec![Dex::RaydiumClV2, Dex::RaydiumCp], vec![Dex::ObricV2, Dex::SolfiV2, Dex::Zerofi, Dex::Humidifi],]
        );
    }

    #[test]
    fn test_parse_nested_dexes_no_quotes_with_spaces() {
        let input = "[[ humidifi , obric-v2 ],[ zerofi ]]";
        let result = CliArgs::parse_nested_dexes(input).unwrap();
        assert_eq!(result, vec![vec![Dex::Humidifi, Dex::ObricV2], vec![Dex::Zerofi]]);
    }

    #[test]
    fn test_parse_nested_dexes_invalid_dex() {
        let input = "[[humidifi,invalid-dex]]";
        let result = CliArgs::parse_nested_dexes(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_nested_dexes_invalid_format() {
        let input = "[humidifi]"; // not nested
        let result = CliArgs::parse_nested_dexes(input);
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
    fn test_dexes_and_weights_match() {
        let dexes_input = "[[humidifi,obric-v2],[zerofi]]";
        let weights_input = "[[50,50],[100]]";

        let dexes = CliArgs::parse_nested_dexes(dexes_input).unwrap();
        let weights = CliArgs::parse_nested_weights(weights_input).unwrap();

        assert_eq!(dexes.len(), weights.len());
        for (d, w) in dexes.iter().zip(weights.iter()) {
            assert_eq!(d.len(), w.len());
        }
    }
}
