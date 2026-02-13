use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Write,
    path::Path,
    str::FromStr,
    time::SystemTime,
};

use base64::{Engine, engine::general_purpose};
use indexmap::IndexMap;
use magnus_shared::Dex;
use serde::Deserialize;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::{account::Account, program_pack::Pack, pubkey::Pubkey, rent::Rent};
use tracing::{info, warn};

use crate::cfg::{Cfg, Keyed};

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

    /// Fetches PMM accounts from an RPC node in a single atomic request.
    ///
    /// Collects all account pubkeys from the provided DEX configurations and fetches
    /// them in one `get_multiple_accounts_with_commitment` call to ensure all accounts
    /// are read at the same slot.
    /// Returns `(slot, { Dex -> [(market_key, [(pubkey, account)])] })`.
    pub fn fetch_accounts(
        pmms: &[Dex],
        client: &RpcClient,
        cfg: &Cfg,
    ) -> eyre::Result<(u64, HashMap<Dex, Vec<(Pubkey, Vec<(Pubkey, Account)>)>>)> {
        let pmms: HashSet<_> = pmms.iter().collect();
        let mut res: HashMap<Dex, Vec<(Pubkey, Vec<(Pubkey, Account)>)>> = HashMap::new();

        let mut all_pubkeys: Vec<Pubkey> = vec![];
        let mut ranges: Vec<(Dex, Pubkey, std::ops::Range<usize>)> = vec![];

        pmms.iter().for_each(|pmm| {
            let markets = cfg.get_accounts(pmm);
            if markets.is_empty() {
                warn!("skipping unsupported prop amm: {pmm}");
                return;
            }

            for (market_key, accs) in markets {
                let start = all_pubkeys.len();
                all_pubkeys.extend(accs.iter());
                let end = all_pubkeys.len();
                ranges.push((**pmm, market_key, start..end));
            }
        });

        let response = client.get_multiple_accounts_with_commitment(&all_pubkeys, CommitmentConfig::confirmed())?;
        let slot = response.context.slot;
        let all_accs = response.value;

        info!("fetched {} accounts for {pmms:?} at slot {slot}", all_pubkeys.len());

        ranges.iter().for_each(|(dex, market_key, range)| {
            let mut market_accs = vec![];

            all_pubkeys[range.clone()].iter().enumerate().for_each(|(i, pubkey)| {
                let idx = range.start + i;
                if let Some(acc) = &all_accs[idx] {
                    market_accs.push((*pubkey, acc.clone()));
                } else {
                    warn!("account {pubkey} not found for {dex} market {market_key}");
                }
            });

            res.entry(*dex).or_default().push((*market_key, market_accs));
        });

        Ok((slot, res))
    }

    /// Parses a Solana account from a JSON file.
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
    pub fn fetch_programs(pmms: &[Dex], client: &RpcClient) -> eyre::Result<Vec<(Dex, Pubkey, Vec<u8>)>> {
        let mut programs = vec![];

        pmms.iter().try_for_each(|pmm| -> eyre::Result<()> {
            let program_id = Pubkey::new_from_array(pmm.program_id().to_bytes());
            let acc = client.get_account(&program_id)?;

            let programdata_pubkey = Pubkey::new_from_array(acc.data[4..36].try_into()?);
            let programdata_acc = client.get_account(&programdata_pubkey)?;

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
    pub fn deser_pubkey<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Pubkey::from_str(&s).map_err(serde::de::Error::custom)
    }

    pub fn deser_market<'de, D, T>(deserializer: D) -> Result<IndexMap<Pubkey, T>, D::Error>
    where
        D: serde::Deserializer<'de>,
        T: Deserialize<'de> + Keyed,
    {
        let items: Vec<T> = Vec::deserialize(deserializer)?;
        Ok(items.into_iter().map(|item| (item.market_key(), item)).collect())
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

#[cfg(test)]
mod tests {
    use solana_sdk::pubkey::Pubkey;

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

        assert_eq!(account.owner, spl_token::id());

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
