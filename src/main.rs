use std::{
    fs,
    fs::File,
    io::{Read, Write},
    path,
    path::{Path, PathBuf},
    str::FromStr,
    time::SystemTime,
    u64,
};

use base64::{Engine, engine::general_purpose};
use litesvm::LiteSVM;
use magnus_router_client::{
    instructions::SwapBuilder,
    types::{Dex, Route},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_sdk::{
    account::Account,
    instruction::Instruction,
    message::{AccountMeta, compiled_instruction::CompiledInstruction},
    program_pack::Pack,
    pubkey,
    pubkey::Pubkey,
    rent::Rent,
    signature::Keypair,
    signer::Signer,
    sysvar,
    transaction::Transaction,
};
use spl_associated_token_account::{get_associated_token_address, instruction::create_associated_token_account_idempotent};
use spl_token::state::AccountState;

pub const WSOL: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
pub const USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
pub const USDT: Pubkey = pubkey!("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB");
pub const SLOT_WARP: u64 = 388417112; // works for solfi-v2 388416639; // works for humidifi - 388408837; // 388406277; //387712188;

pub mod solfi_v2 {
    use solana_sdk::{pubkey, pubkey::Pubkey};

    pub const MARKET: Pubkey = pubkey!("65ZHSArs5XxPseKQbB1B4r16vDxMWnCxHMzogDAqiDUc");
    pub const POOL_BASE_VAULT: Pubkey = pubkey!("CRo8DBwrmd97DJfAnvCv96tZPL5Mktf2NZy2ZnhDer1A");
    pub const POOL_QUOTE_VAULT: Pubkey = pubkey!("GhFfLFSprPpfoRaWakPMmJTMJBHuz6C694jYwxy2dAic");
    pub const CFG: Pubkey = pubkey!("FmxXDSR9WvpJTCh738D1LEDuhMoA8geCtZgHb3isy7Dp");
    pub const ORACLE: Pubkey = pubkey!("2ny7eGyZCoeEVTkNLf5HcnJFBKkyA4p4gcrtb3b8y8ou");
    pub const SLOT: u64 = 388416639;
}

pub mod humidifi {
    use solana_sdk::{pubkey, pubkey::Pubkey};

    pub const MARKET: Pubkey = pubkey!("DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW");
    pub const BASE_TOKEN_ACCOUNT: Pubkey = pubkey!("8BrVfsvzb1DZqCactbYWoKSv24AfsLBuXJqzpzYCwznF");
    pub const QUOTE_TOKEN_ACCOUNT: Pubkey = pubkey!("HsQcHFFNUVTp3MWrXYbuZchBNd4Pwk8636bKzLvpfYNR");
    pub const SLOT: u64 = 388417112;
}

pub mod zerofi {
    use solana_sdk::{pubkey, pubkey::Pubkey};

    pub const PAIR: Pubkey = pubkey!("2h9hhu3gxY9kCdXEwdTHV8yPAMYVoHgKopRyG1HbDwfi");
    pub const VAULT_INFO_BASE: Pubkey = pubkey!("7RHJ2WfexqUxy7SXfbNZRZDgZi3D9jtMAQp9VhfzpU8T");
    pub const VAULT_BASE: Pubkey = pubkey!("ERP5RTV6cWmoGrv7r9W2V5pbgDFSepc4j97qNnx1Jris");
    pub const VAULT_INFO_QUOTE: Pubkey = pubkey!("Ef7zPqj4NuZHwaTczUTY9oRbxXrfZseUcKcqPaidCZ5W");
    pub const VAULT_QUOTE: Pubkey = pubkey!("7wYJVD8iXmMQjND1fwi1hPr68QwruVVtirbotyJZXaVH");

    pub const SLOT: u64 = 388487328;
}

pub mod obric_v2 {
    use solana_sdk::{pubkey, pubkey::Pubkey};

    pub const TRADING_PAIR: Pubkey = pubkey!("BWBHrYqfcjAh5dSiRwzPnY4656cApXVXmkeDmAfwBKQG");
    pub const SECOND_REFERENCE_ORACLE: Pubkey = pubkey!("GZsNmWKbqhMYtdSkkvMdEyQF9k5mLmP7tTKYWZjcHVPE");
    pub const THIRD_REFERENCE_ORACLE: Pubkey = pubkey!("6YawcNeZ74tRyCv4UfGydYMr7eho7vbUR6ScVffxKAb3");
    pub const RESERVE_X: Pubkey = pubkey!("C3tPQ8TRcHybnPpR8KMASUVD3PukQRRHEsLwxorJMhgm");
    pub const RESERVE_Y: Pubkey = pubkey!("AAamGhyPfpQJWfZHTq944NM1cFvoVLDrQxt7HGjeRQUS");
    pub const REFERENCE_ORACLE: Pubkey = pubkey!("J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw");
    pub const X_PRICE_FEED: Pubkey = pubkey!("J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw");
    pub const Y_PRICE_FEED: Pubkey = pubkey!("J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw");

    pub const SLOT: u64 = 388491516;
}

pub fn setup_sim_env() -> eyre::Result<LiteSVM> {
    let mut budget = ComputeBudget::new_with_defaults(false);
    budget.compute_unit_limit = 2_000_000;
    let mut env = LiteSVM::new().with_default_programs().with_sysvars().with_sigverify(true).with_compute_budget(budget);

    env.add_program_from_file(magnus_router_client::programs::ROUTER_ID, "cfg/magnus-router.so")?;
    env.add_program_from_file(Pubkey::from_str(&magnus_consts::pmm_zerofi::id().to_string())?, "cfg/zerofi.so")?;
    env.add_program_from_file(Pubkey::from_str(&magnus_consts::pmm_humidifi::id().to_string())?, "cfg/humidifi.so")?;
    env.add_program_from_file(Pubkey::from_str(&magnus_consts::pmm_solfi_v2::id().to_string())?, "cfg/solfi-v2.so")?;
    env.add_program_from_file(Pubkey::from_str(&magnus_consts::pmm_obric_v2::id().to_string())?, "cfg/obric-v2.so")?;

    env.set_account(WSOL, create_mint_account(9))?;
    env.set_account(USDT, create_mint_account(6))?;
    env.set_account(USDC, create_mint_account(6))?;

    env.warp_to_slot(obric_v2::SLOT);

    Ok(env)
}

pub fn setup_wallet(env: &mut LiteSVM) -> eyre::Result<Keypair> {
    let wallet = Keypair::new();

    let wallet_ata_wsol = get_associated_token_address(&wallet.pubkey(), &WSOL);
    let wallet_ata_usdt = get_associated_token_address(&wallet.pubkey(), &USDT);
    let wallet_ata_usdc = get_associated_token_address(&wallet.pubkey(), &USDC);

    let wallet_wsol_acc = mk_ata_account(&WSOL, &wallet.pubkey(), 100_000_000_000);
    let wallet_usdt_acc = mk_ata_account(&USDT, &wallet.pubkey(), 100_000_000_000);
    let wallet_usdc_acc = mk_ata_account(&USDC, &wallet.pubkey(), 100_000_000_000);

    env.set_account(wallet_ata_wsol, wallet_wsol_acc)?;
    env.set_account(wallet_ata_usdt, wallet_usdt_acc)?;
    env.set_account(wallet_ata_usdc, wallet_usdc_acc)?;
    let _ = env.airdrop(&wallet.pubkey(), 100_000_000_000);

    Ok(wallet)
}

fn main() -> eyre::Result<()> {
    let mut env = setup_sim_env()?;
    let wallet = setup_wallet(&mut env)?;

    for acct in AccountWithAddress::read_all(Some("solfi-v2".to_string()))? {
        env.set_account(acct.pubkey, acct.account)?;
    }
    for acct in AccountWithAddress::read_all(Some("obric-v2".to_string()))? {
        env.set_account(acct.pubkey, acct.account)?;
    }

    let wallet_ata_wsol = get_associated_token_address(&wallet.pubkey(), &WSOL);
    let wallet_ata_usdt = get_associated_token_address(&wallet.pubkey(), &USDT);
    let wallet_ata_usdc = get_associated_token_address(&wallet.pubkey(), &USDC);
    let wsol_balance_before = token_balance(&env, &wallet_ata_wsol);
    let usdt_balance_before = token_balance(&env, &wallet_ata_usdt);
    let usdc_balance_before = token_balance(&env, &wallet_ata_usdc);

    println!("wsol_balance_before: {wsol_balance_before} | usdt_balance_before: {usdt_balance_before} | usdc_balance_before: {usdc_balance_before}");

    let mut swap_binding = SwapBuilder::new();
    let swap = swap_binding
        .payer(wallet.pubkey())
        .source_token_account(wallet_ata_usdt)
        .destination_token_account(wallet_ata_usdc)
        .source_mint(WSOL)
        .destination_mint(USDC)
        .amount_in(1000_000_000)
        .expect_amount_out(1)
        .min_return(1)
        .amounts(vec![1000_000_000])
        .routes(vec![
            vec![Route { dexes: vec![Dex::ObricV2], weights: vec![100] }],
            //
            //vec![Route { dexes: vec![Dex::SolfiV2], weights: vec![100] }],
            //vec![Route { dexes: vec![Dex::SolfiV2, Dex::Humidifi], weights: vec![50, 50] }],
        ])
        .order_id(SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());

    let mut construct = ConstructSwap { builder: swap, payer: wallet.pubkey(), sta: wallet_ata_usdt, dta: wallet_ata_usdc };
    construct.attach_obric_v2_accs(None, None, None);
    //construct.attach_zerofi_accs();
    //construct.attach_solfiv2_accs();
    //construct.attach_solfiv2_accs();
    //construct.attach_humidifi_accs();
    let swap_ix = construct.instruction();

    let ix = vec![swap_ix];
    let tx = Transaction::new_signed_with_payer(&ix, Some(&wallet.pubkey()), &[&wallet], env.latest_blockhash());
    let sig = env.send_transaction(tx);
    println!("{sig:#?}");

    let wsol_balance_after = token_balance(&env, &wallet_ata_wsol);
    let usdt_balance_after = token_balance(&env, &wallet_ata_usdt);
    let usdc_balance_after = token_balance(&env, &wallet_ata_usdc);

    println!("wsol_balance_after: {wsol_balance_after} | usdt_balance_after: {usdt_balance_after} | usdc_balance_after: {usdc_balance_after}");

    Ok(())
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

    pub fn attach_solfiv2_accs(&mut self, payer: Option<Pubkey>, sta: Option<Pubkey>, dta: Option<Pubkey>) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(Pubkey::from_str(&magnus_consts::pmm_solfi_v2::id().to_string()).unwrap(), false))
            .add_remaining_account(AccountMeta::new(payer.unwrap_or(self.payer), true))
            .add_remaining_account(AccountMeta::new(sta.unwrap_or(self.sta), false))
            .add_remaining_account(AccountMeta::new(dta.unwrap_or(self.dta), false))
            .add_remaining_account(AccountMeta::new(solfi_v2::MARKET, false))
            .add_remaining_account(AccountMeta::new_readonly(solfi_v2::ORACLE, false))
            .add_remaining_account(AccountMeta::new_readonly(solfi_v2::CFG, false))
            .add_remaining_account(AccountMeta::new(solfi_v2::POOL_BASE_VAULT, false))
            .add_remaining_account(AccountMeta::new(solfi_v2::POOL_QUOTE_VAULT, false))
            .add_remaining_account(AccountMeta::new_readonly(WSOL, false))
            .add_remaining_account(AccountMeta::new_readonly(USDC, false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::instructions::id(), false));
    }

    pub fn attach_humidifi_accs(&mut self, payer: Option<Pubkey>, sta: Option<Pubkey>, dta: Option<Pubkey>) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(Pubkey::from_str(&magnus_consts::pmm_humidifi::id().to_string()).unwrap(), false))
            .add_remaining_account(AccountMeta::new(payer.unwrap_or(self.payer), true))
            .add_remaining_account(AccountMeta::new(sta.unwrap_or(self.sta), false))
            .add_remaining_account(AccountMeta::new(dta.unwrap_or(self.dta), false))
            .add_remaining_account(AccountMeta::new_readonly(create_humidifi_param(1500), false))
            .add_remaining_account(AccountMeta::new(humidifi::MARKET, false))
            .add_remaining_account(AccountMeta::new(humidifi::BASE_TOKEN_ACCOUNT, false))
            .add_remaining_account(AccountMeta::new(humidifi::QUOTE_TOKEN_ACCOUNT, false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::clock::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::instructions::id(), false));
    }

    pub fn attach_zerofi_accs(&mut self, payer: Option<Pubkey>, sta: Option<Pubkey>, dta: Option<Pubkey>) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(Pubkey::from_str(&magnus_consts::pmm_zerofi::id().to_string()).unwrap(), false))
            .add_remaining_account(AccountMeta::new(payer.unwrap_or(self.payer), true))
            .add_remaining_account(AccountMeta::new(sta.unwrap_or(self.sta), false))
            .add_remaining_account(AccountMeta::new(dta.unwrap_or(self.dta), false))
            .add_remaining_account(AccountMeta::new(zerofi::PAIR, false))
            .add_remaining_account(AccountMeta::new(zerofi::VAULT_INFO_BASE, false))
            .add_remaining_account(AccountMeta::new(zerofi::VAULT_BASE, false))
            .add_remaining_account(AccountMeta::new(zerofi::VAULT_INFO_QUOTE, false))
            .add_remaining_account(AccountMeta::new(zerofi::VAULT_QUOTE, false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false))
            .add_remaining_account(AccountMeta::new_readonly(sysvar::instructions::id(), false));
    }

    pub fn attach_obric_v2_accs(&mut self, payer: Option<Pubkey>, sta: Option<Pubkey>, dta: Option<Pubkey>) {
        self.builder
            .add_remaining_account(AccountMeta::new_readonly(Pubkey::from_str(&magnus_consts::pmm_obric_v2::id().to_string()).unwrap(), false))
            .add_remaining_account(AccountMeta::new(payer.unwrap_or(self.payer), true))
            .add_remaining_account(AccountMeta::new(sta.unwrap_or(self.sta), false))
            .add_remaining_account(AccountMeta::new(dta.unwrap_or(self.dta), false))
            .add_remaining_account(AccountMeta::new(obric_v2::TRADING_PAIR, false))
            .add_remaining_account(AccountMeta::new_readonly(obric_v2::SECOND_REFERENCE_ORACLE, false))
            .add_remaining_account(AccountMeta::new_readonly(obric_v2::THIRD_REFERENCE_ORACLE, false))
            .add_remaining_account(AccountMeta::new(obric_v2::RESERVE_X, false))
            .add_remaining_account(AccountMeta::new(obric_v2::RESERVE_Y, false))
            .add_remaining_account(AccountMeta::new(obric_v2::REFERENCE_ORACLE, false))
            .add_remaining_account(AccountMeta::new_readonly(obric_v2::X_PRICE_FEED, false))
            .add_remaining_account(AccountMeta::new_readonly(obric_v2::Y_PRICE_FEED, false))
            .add_remaining_account(AccountMeta::new_readonly(spl_token::id(), false));
    }
}

fn mk_ata_account(mint: &Pubkey, user: &Pubkey, amount: u64) -> Account {
    let ata = spl_token::state::Account { mint: *mint, owner: *user, amount, state: spl_token::state::AccountState::Initialized, ..Default::default() };
    let mut data = vec![0u8; spl_token::state::Account::LEN];
    ata.pack_into_slice(&mut data);
    Account { lamports: Rent::default().minimum_balance(data.len()), data, owner: spl_token::id(), executable: false, rent_epoch: u64::MAX }
}

#[derive(Serialize, Deserialize)]
pub struct AccountWithAddress {
    pub pubkey: Pubkey,
    pub account: Account,
    pub prefix: String,
}

impl AccountWithAddress {
    fn get_filename(&self) -> String {
        format!("{}_{}.json", self.prefix, self.pubkey)
    }

    pub fn save_to_file(&self) -> eyre::Result<()> {
        let filename = self.get_filename();
        let serialized = serde_json::to_string(self)?;
        let data_dir = Path::new("cfg");
        if !data_dir.exists() {
            fs::create_dir(data_dir)?;
        }
        let file_path = data_dir.join(filename);
        let mut file = File::create(file_path)?;
        file.write_all(serialized.as_bytes())?;

        Ok(())
    }

    pub fn read_account(path: PathBuf, prefix: String) -> eyre::Result<AccountWithAddress> {
        let contents = fs::read_to_string(&path)?;
        let value: serde_json::Value = serde_json::from_str(&contents)?;

        let pubkey = Pubkey::from_str(value["pubkey"].as_str().unwrap())?;
        let lamports = value["account"]["lamports"].as_u64().unwrap();
        let data_base64 = value["account"]["data"][0].as_str().unwrap();
        let data = general_purpose::STANDARD.decode(data_base64)?;
        let owner = Pubkey::from_str(value["account"]["owner"].as_str().unwrap())?;
        let executable = value["account"]["executable"].as_bool().unwrap();
        let rent_epoch = value["account"]["rentEpoch"].as_u64().unwrap();

        Ok(AccountWithAddress { pubkey, account: Account { lamports, data, owner, executable, rent_epoch }, prefix })
    }

    pub fn read_all(prefix: Option<String>) -> eyre::Result<Vec<Self>> {
        let data_dir = Path::new("cfg");
        if !data_dir.exists() {
            return Ok(vec![]);
        }

        let mut accounts = Vec::new();

        for entry in fs::read_dir(data_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.file_name().and_then(|n| n.to_str()).is_some_and(|name| name.starts_with(&prefix.clone().unwrap_or_default()) && name.ends_with(".json")) {
                accounts.push(Self::read_account(path, prefix.clone().unwrap_or_default())?);
            }
        }

        Ok(accounts)
    }
}

pub fn token_balance(svm: &LiteSVM, pubkey: &Pubkey) -> u64 {
    let account = svm.get_account(pubkey).unwrap_or_default();
    let state = spl_token::state::Account::unpack(&account.data).ok().unwrap_or_default();
    state.amount
}

fn create_mint_account(decimals: u8) -> Account {
    let mint = spl_token::state::Mint {
        mint_authority: solana_sdk::program_option::COption::None,
        supply: u64::MAX,
        decimals,
        is_initialized: true,
        freeze_authority: Default::default(),
    };

    let mut data = vec![0u8; spl_token::state::Mint::LEN];
    spl_token::state::Mint::pack(mint, &mut data).unwrap();

    Account { lamports: Rent::default().minimum_balance(data.len()), data, owner: spl_token::id(), executable: false, rent_epoch: u64::MAX }
}

fn create_humidifi_param(swap_id: u64) -> Pubkey {
    let mut bytes = [0u8; 32];
    bytes[0..8].copy_from_slice(&swap_id.to_le_bytes());
    Pubkey::new_from_array(bytes)
}
