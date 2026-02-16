use std::fmt::{Debug, Display};

use litesvm::{LiteSVM, types::TransactionMetadata};
use magnus_router_client::types::SwapEvent;
use magnus_shared::Dex;
use solana_client::rpc_client::RpcClient;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_sdk::{account::Account, program_pack::Pack, pubkey::Pubkey, signature::Keypair, signer::Signer, transaction::Transaction};
use spl_associated_token_account::get_associated_token_address;

use crate::{Aggregator, Misc, cfg::Cfg, consts};

/// The Simulation Environment;
/// Ensures proper setup of LiteSVM, wallet, programs, and accounts.
/// Also provides utility functions for common operations, like
/// loading programs/accounts, setting up the wallet, sending transactions, etc.
pub struct Environment<'a, P: Into<String> + Display + Clone + Debug> {
    svm: LiteSVM,
    pub slot: Option<u64>,
    pub wallet: Keypair,
    mints: Option<&'a [(Pubkey, u8)]>,
    cfg: Cfg,

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
        cfg: Cfg,
        slot: Option<u64>,
    ) -> eyre::Result<Environment<'_, P>> {
        let mut budget = ComputeBudget::new_with_defaults(false, false);
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
    pub fn setup_wallet(&mut self, src_mint: &Pubkey, src_amount: u64, airdrop_amount: u64) -> eyre::Result<&mut Self> {
        self.reset_wallet(src_mint, src_amount)?;
        self.svm.airdrop(&self.wallet_pubkey(), airdrop_amount).expect("airdrop failed");

        Ok(self)
    }

    /// Resets the wallet's token balances between simulation iterations.
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
    pub fn get_and_load_programs(
        &mut self,
        pmms: &[Dex],
        jit: bool,
        spoof: Option<Aggregator>,
        client: Option<&RpcClient>,
    ) -> eyre::Result<&mut Self> {
        let pmms: Vec<Dex> = pmms.iter().copied().collect::<std::collections::HashSet<_>>().into_iter().collect();
        self.load_program_router(spoof)?;

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

        Ok(self)
    }

    /// Loads the router program and all required PMM programs from disk.
    pub fn static_programs(&mut self, pmms: &[Dex]) -> eyre::Result<&mut Self> {
        let programs = Misc::read_programs_from_disk(pmms, &self.programs_path.to_string())?;

        programs.into_iter().try_for_each(|(program_id, path)| self.svm.add_program_from_file(program_id, path))?;

        Ok(self)
    }

    /// Loads the router program into the SVM.
    pub fn load_program_router(&mut self, spoof: Option<Aggregator>) -> eyre::Result<&mut Self> {
        match spoof {
            None => {
                self.svm.add_program_from_file(
                    magnus_router_client::programs::ROUTER_ID,
                    format!("{}/{}.so", self.programs_path, consts::ROUTER),
                )?;
            }
            Some(aggr) => {
                let addr = aggr.program_id();
                self.svm.add_program_from_file(addr, format!("{}/{}-spoof-{aggr}.so", self.programs_path, consts::ROUTER))?;
            }
        }

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

        accs_map.iter().try_for_each(|(_, markets)| markets.iter().try_for_each(|(_, accs)| self.set_accounts(accs).map(|_| ())))?;

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

    /// Extracts all SwapEvents from a swap transaction's logs.
    ///
    /// Parses the `SwapEvent` logs emitted by the router program to find
    /// `dex`, `amount_in`, and `amount_out` values. Panics if no events are found.
    pub fn get_swap_events(&self, metadata: &TransactionMetadata) -> Vec<SwapEvent> {
        let events: Vec<SwapEvent> = metadata
            .logs
            .iter()
            .filter_map(|log| {
                if !log.contains("SwapEvent") {
                    return None;
                }
                let dex_str = log.split("dex: ").nth(1)?.split(',').next()?;
                let dex: Dex = dex_str.to_lowercase().parse().ok()?;
                let amount_in = log.split("amount_in: ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
                let amount_out = log.split("amount_out: ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
                Some(SwapEvent { dex: dex.into(), amount_in, amount_out })
            })
            .collect();

        assert!(!events.is_empty(), "couldn't find any SwapEvent in logs");
        events
    }

    /// Extracts the final destination balance from a swap transaction's logs.
    ///
    /// Parses the `after_destination_balance` value from the router's balance log.
    pub fn get_amount_out(&self, metadata: &TransactionMetadata) -> u64 {
        metadata
            .logs
            .iter()
            .find_map(|log| log.split("after_destination_balance: ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok())
            .expect("couldn't find after_destination_balance in logs")
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::{account::Account, program_pack::Pack, pubkey, pubkey::Pubkey};
    use spl_associated_token_account::get_associated_token_address;

    use super::*;

    const WSOL: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
    const WSOL_DECIMALS: u8 = 9;
    const USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    const USDC_DECIMALS: u8 = 6;

    fn default_cfg() -> Cfg {
        toml::from_str("").unwrap()
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
        let mints = vec![(WSOL, WSOL_DECIMALS), (USDC, USDC_DECIMALS)];

        let env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

        assert!(env.mints.is_some());
        assert_eq!(env.mints.unwrap().len(), 2);

        let wsol_account = env.svm.get_account(&WSOL);
        let usdc_account = env.svm.get_account(&USDC);

        assert!(wsol_account.is_some());
        assert!(usdc_account.is_some());

        let wsol_mint = spl_token::state::Mint::unpack(&wsol_account.unwrap().data).unwrap();
        let usdc_mint = spl_token::state::Mint::unpack(&usdc_account.unwrap().data).unwrap();

        assert_eq!(wsol_mint.decimals, WSOL_DECIMALS);
        assert_eq!(usdc_mint.decimals, USDC_DECIMALS);
        assert!(wsol_mint.is_initialized);
        assert!(usdc_mint.is_initialized);
    }

    #[test]
    fn test_wallet_ata_derives_correct_address() {
        let mints = vec![(WSOL, WSOL_DECIMALS)];
        let env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

        let expected_ata = get_associated_token_address(&env.wallet_pubkey(), &WSOL);
        let actual_ata = env.wallet_ata(&WSOL);

        assert_eq!(actual_ata, expected_ata);
    }

    #[test]
    fn test_setup_wallet_creates_atas_and_funds() {
        let mints = vec![(WSOL, WSOL_DECIMALS), (USDC, USDC_DECIMALS)];
        let mut env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

        let src_amount = 1_000_000_000u64;
        let airdrop = 10_000_000_000u64;

        env.setup_wallet(&WSOL, src_amount, airdrop).unwrap();

        assert_eq!(env.token_balance(&WSOL), src_amount);
        assert_eq!(env.token_balance(&USDC), 0);

        let wallet_account = env.svm.get_account(&env.wallet_pubkey());
        assert!(wallet_account.is_some());
        assert!(wallet_account.unwrap().lamports >= airdrop);
    }

    #[test]
    fn test_reset_wallet_restores_balances() {
        let mints = vec![(WSOL, WSOL_DECIMALS), (USDC, USDC_DECIMALS)];
        let mut env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

        env.setup_wallet(&WSOL, 1_000_000_000, 10_000_000_000).unwrap();

        let wsol_ata = env.wallet_ata(&WSOL);
        let usdc_ata = env.wallet_ata(&USDC);
        env.svm.set_account(wsol_ata, Misc::mk_ata(&WSOL, &env.wallet_pubkey(), 500_000_000)).unwrap();
        env.svm.set_account(usdc_ata, Misc::mk_ata(&USDC, &env.wallet_pubkey(), 100_000_000)).unwrap();

        let new_amount = 2_000_000_000u64;
        env.reset_wallet(&WSOL, new_amount).unwrap();

        assert_eq!(env.token_balance(&WSOL), new_amount);
        assert_eq!(env.token_balance(&USDC), 0);
    }

    #[test]
    fn test_token_balance_returns_zero_for_nonexistent_ata() {
        let env = Environment::new("", "", None, default_cfg(), None).unwrap();

        assert_eq!(env.token_balance(&WSOL), 0);
    }

    #[test]
    fn test_token_balance_norm_converts_correctly() {
        let mints = vec![(WSOL, WSOL_DECIMALS)];
        let mut env = Environment::new("", "", Some(&mints), default_cfg(), None).unwrap();

        let raw_amount = 1_500_000_000u64;
        env.setup_wallet(&WSOL, raw_amount, 10_000_000_000).unwrap();

        let normalized = env.token_balance_norm(&WSOL, WSOL_DECIMALS);

        assert!((normalized - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_latest_blockhash_returns_valid_hash() {
        let env = Environment::new("", "", None, default_cfg(), None).unwrap();

        let blockhash = env.latest_blockhash();

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

    fn mock_metadata(logs: Vec<&str>) -> TransactionMetadata {
        TransactionMetadata { logs: logs.into_iter().map(String::from).collect(), ..Default::default() }
    }

    #[test]
    fn test_get_swap_events_parses_all_events() {
        let env = Environment::new("", "", None, default_cfg(), None).unwrap();
        let metadata = mock_metadata(vec![
            "Program data: QMbN6CYIceIFgHWE3wAAAACC1uW4CQAAAA==",
            "Program log: SwapEvent { dex: HumidiFi, amount_in: 3750000000, amount_out: 41756776066 }",
            "Program log: CUX1SEkh3HmNqv6zzkXFr6VsxGHa66jt98E3qBtXoEni",
            "Program log: Dex::Humidifi amount_in: 6000000000, offset: 27",
            "Program data: QMbN6CYIceIFALygZQEAAADSwRWODwAAAA==",
            "Program log: SwapEvent { dex: HumidiFi, amount_in: 6000000000, amount_out: 66808299986 }",
            "Program log: CUX1SEkh3HmNqv6zzkXFr6VsxGHa66jt98E3qBtXoEni",
        ]);

        let events = env.get_swap_events(&metadata);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].dex, magnus_router_client::types::Dex::HumidiFi);
        assert_eq!(events[0].amount_in, 3750000000);
        assert_eq!(events[0].amount_out, 41756776066);
        assert_eq!(events[1].dex, magnus_router_client::types::Dex::HumidiFi);
        assert_eq!(events[1].amount_in, 6000000000);
        assert_eq!(events[1].amount_out, 66808299986);
    }

    #[test]
    #[should_panic(expected = "couldn't find any SwapEvent in logs")]
    fn test_get_swap_events_panics_when_no_events() {
        let env = Environment::new("", "", None, default_cfg(), None).unwrap();
        let metadata = mock_metadata(vec!["Program log: some unrelated log"]);

        env.get_swap_events(&metadata);
    }

    #[test]
    fn test_get_amount_out_parses_after_destination_balance() {
        let env = Environment::new("", "", None, default_cfg(), None).unwrap();
        let metadata = mock_metadata(vec![
            "Program log: SwapEvent { dex: HumidiFi, amount_in: 6000000000, amount_out: 66808299986 }",
            "Program log: after_source_balance: 0, after_destination_balance: 167066576506, source_token_change: 15000000000, \
             destination_token_change: 167066576506",
        ]);

        assert_eq!(env.get_amount_out(&metadata), 167066576506);
    }

    #[test]
    #[should_panic(expected = "couldn't find after_destination_balance in logs")]
    fn test_get_amount_out_panics_when_missing() {
        let env = Environment::new("", "", None, default_cfg(), None).unwrap();
        let metadata = mock_metadata(vec!["Program log: some unrelated log"]);

        env.get_amount_out(&metadata);
    }
}
