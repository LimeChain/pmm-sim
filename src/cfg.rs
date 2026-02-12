use std::{collections::HashMap, fmt, fs, str::FromStr};

use magnus_shared::Dex;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;

use crate::Misc;

/// A CLI-facing target: a DEX plus an optional market hint.
///
/// Parsed from strings like `"humidifi"` (first market) or `"humidifi_Fksf"`
/// (market whose pubkey starts with `Fksf`).
#[derive(Debug, Clone)]
pub struct PmmTarget {
    pub dex: Dex,
    pub market_hint: Option<String>,
}

impl PmmTarget {
    pub fn resolve(&self, cfg: &Cfg) -> Option<Pubkey> {
        let markets = cfg.get_accounts(&self.dex);
        match &self.market_hint {
            None => markets.first().map(|(k, _)| *k),
            Some(hint) => markets.iter().find(|(k, _)| k.to_string().starts_with(hint.as_str())).map(|(k, _)| *k),
        }
    }
}

impl FromStr for PmmTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((dex_part, hint)) = s.rsplit_once('_') {
            match dex_part.parse::<Dex>() {
                Ok(dex) => Ok(PmmTarget { dex, market_hint: Some(hint.to_string()) }),
                Err(_) => {
                    let dex = s.parse::<Dex>()?;
                    Ok(PmmTarget { dex, market_hint: None })
                }
            }
        } else {
            let dex = s.parse::<Dex>()?;
            Ok(PmmTarget { dex, market_hint: None })
        }
    }
}

impl fmt::Display for PmmTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.market_hint {
            Some(hint) => write!(f, "{}_{}", self.dex, hint),
            None => write!(f, "{}", self.dex),
        }
    }
}

pub trait Keyed {
    fn market_key(&self) -> Pubkey;
}

pub fn vec_to_map<'de, D, T>(deserializer: D) -> Result<HashMap<Pubkey, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Keyed,
{
    let items: Vec<T> = Vec::deserialize(deserializer)?;
    Ok(items.into_iter().map(|item| (item.market_key(), item)).collect())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Cfg {
    pub humidifi: Option<HumidifiCfg>,
    pub tessera: Option<TesseraCfg>,
    pub goonfi: Option<GoonfiCfg>,
    pub solfi_v2: Option<SolfiV2Cfg>,
    pub zerofi: Option<ZerofiCfg>,
    pub obric_v2: Option<ObricV2Cfg>,
    pub bisonfi: Option<BisonfiCfg>,
}

impl Cfg {
    pub fn load(path: &str) -> eyre::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let cfg: Cfg = toml::from_str(&contents)?;
        Ok(cfg)
    }

    /// Returns all (market_key, account_pubkeys) pairs for the given DEX.
    pub fn get_accounts(&self, dex: &Dex) -> Vec<(Pubkey, Vec<Pubkey>)> {
        macro_rules! collect_markets {
            ($field:expr) => {
                $field.as_ref().map_or_else(Vec::new, |cfg| cfg.swap_v1.iter().map(|(k, v)| (*k, v.accounts())).collect())
            };
        }

        match dex {
            Dex::HumidiFi => collect_markets!(self.humidifi),
            Dex::Tessera => collect_markets!(self.tessera),
            Dex::GoonFi => collect_markets!(self.goonfi),
            Dex::SolfiV2 => collect_markets!(self.solfi_v2),
            Dex::ZeroFi => collect_markets!(self.zerofi),
            Dex::ObricV2 => collect_markets!(self.obric_v2),
            Dex::BisonFi => collect_markets!(self.bisonfi),
            _ => vec![],
        }
    }

    /// Returns the first market pubkey for the given DEX (for labeling / backward compat).
    pub fn get_market(&self, dex: &Dex) -> Option<Pubkey> {
        self.get_accounts(dex).first().map(|(k, _)| *k)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HumidifiCfg {
    #[serde(default, deserialize_with = "vec_to_map")]
    pub swap_v1: HashMap<Pubkey, HumidifiSwapV1>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HumidifiSwapV1 {
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub quote_ta: Pubkey,
}

impl HumidifiSwapV1 {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.base_ta, self.quote_ta]
    }
}

impl Keyed for HumidifiSwapV1 {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

// -- Tessera --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TesseraCfg {
    #[serde(default, deserialize_with = "vec_to_map")]
    pub swap_v1: HashMap<Pubkey, TesseraMarket>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TesseraMarket {
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub quote_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub global_state: Pubkey,
}

impl TesseraMarket {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.base_ta, self.quote_ta, self.global_state]
    }
}

impl Keyed for TesseraMarket {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

// -- Goonfi --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GoonfiCfg {
    #[serde(default, deserialize_with = "vec_to_map")]
    pub swap_v1: HashMap<Pubkey, GoonfiMarket>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GoonfiMarket {
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub quote_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub blacklist: Pubkey,
}

impl GoonfiMarket {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.base_ta, self.quote_ta, self.blacklist]
    }
}

impl Keyed for GoonfiMarket {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

// -- SolFi V2 --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SolfiV2Cfg {
    #[serde(default, deserialize_with = "vec_to_map")]
    pub swap_v1: HashMap<Pubkey, SolfiV2Market>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SolfiV2Market {
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub quote_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub cfg: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub base_mint: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub quote_mint: Pubkey,
}

impl SolfiV2Market {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.base_ta, self.quote_ta, self.cfg, self.oracle, self.base_mint, self.quote_mint]
    }
}

impl Keyed for SolfiV2Market {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

// -- ZeroFi --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ZerofiCfg {
    #[serde(default, deserialize_with = "vec_to_map")]
    pub swap_v1: HashMap<Pubkey, ZerofiMarket>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ZerofiMarket {
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub vault_info_base: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub vault_base: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub vault_info_quote: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub vault_quote: Pubkey,
}

impl ZerofiMarket {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.vault_info_base, self.vault_base, self.vault_info_quote, self.vault_quote]
    }
}

impl Keyed for ZerofiMarket {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

// -- Obric V2 --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ObricV2Cfg {
    #[serde(default, deserialize_with = "vec_to_map")]
    pub swap_v1: HashMap<Pubkey, ObricV2Market>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ObricV2Market {
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub second_ref_oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub third_ref_oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub reserve_x: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub reserve_y: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub ref_oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub x_price_feed: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub y_price_feed: Pubkey,
}

impl ObricV2Market {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![
            self.market,
            self.second_ref_oracle,
            self.third_ref_oracle,
            self.reserve_x,
            self.reserve_y,
            self.ref_oracle,
            self.x_price_feed,
            self.y_price_feed,
        ]
    }
}

impl Keyed for ObricV2Market {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

// -- BisonFi --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BisonfiCfg {
    #[serde(default, deserialize_with = "vec_to_map")]
    pub swap_v1: HashMap<Pubkey, BisonfiMarket>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BisonfiMarket {
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market_base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deserialize_pubkey")]
    pub market_quote_ta: Pubkey,
}

impl BisonfiMarket {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.market_base_ta, self.market_quote_ta]
    }
}

impl Keyed for BisonfiMarket {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn pk(s: &str) -> Pubkey {
        Pubkey::from_str(s).unwrap()
    }

    #[test]
    fn parse_single_market() {
        let toml = r#"
            [humidifi]
            [[humidifi.swap-v1]]
            market = "FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"
            base_ta = "C3FzbX9n1YD2dow2dCmEv5uNyyf22Gb3TLAEqGBhw5fY"
            quote_ta = "3RWFAQBRkNGq7CMGcTLK3kXDgFTe9jgMeFYqk8nHwcWh"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let humidifi = cfg.humidifi.unwrap();

        assert_eq!(humidifi.swap_v1.len(), 1);

        let market_pk = pk("FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH");
        assert!(humidifi.swap_v1.contains_key(&market_pk));

        let entry = &humidifi.swap_v1[&market_pk];
        assert_eq!(entry.market, market_pk);
        assert_eq!(entry.base_ta, pk("C3FzbX9n1YD2dow2dCmEv5uNyyf22Gb3TLAEqGBhw5fY"));
        assert_eq!(entry.quote_ta, pk("3RWFAQBRkNGq7CMGcTLK3kXDgFTe9jgMeFYqk8nHwcWh"));
    }

    #[test]
    fn parse_multiple_markets() {
        let toml = r#"
            [humidifi]
            [[humidifi.swap-v1]]
            market = "FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"
            base_ta = "C3FzbX9n1YD2dow2dCmEv5uNyyf22Gb3TLAEqGBhw5fY"
            quote_ta = "3RWFAQBRkNGq7CMGcTLK3kXDgFTe9jgMeFYqk8nHwcWh"

            [[humidifi.swap-v1]]
            market = "DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW"
            base_ta = "8BrVfsvzb1DZqCactbYWoKSv24AfsLBuXJqzpzYCwznF"
            quote_ta = "HsQcHFFNUVTp3MWrXYbuZchBNd4Pwk8636bKzLvpfYNR"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let markets = cfg.humidifi.unwrap().swap_v1;

        assert_eq!(markets.len(), 2);
        assert!(markets.contains_key(&pk("FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH")));
        assert!(markets.contains_key(&pk("DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW")));
    }

    #[test]
    fn lookup_by_prefix() {
        let toml = r#"
            [humidifi]
            [[humidifi.swap-v1]]
            market = "FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"
            base_ta = "C3FzbX9n1YD2dow2dCmEv5uNyyf22Gb3TLAEqGBhw5fY"
            quote_ta = "3RWFAQBRkNGq7CMGcTLK3kXDgFTe9jgMeFYqk8nHwcWh"

            [[humidifi.swap-v1]]
            market = "DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW"
            base_ta = "8BrVfsvzb1DZqCactbYWoKSv24AfsLBuXJqzpzYCwznF"
            quote_ta = "HsQcHFFNUVTp3MWrXYbuZchBNd4Pwk8636bKzLvpfYNR"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let markets = cfg.humidifi.unwrap().swap_v1;

        let prefix = "Fksf";
        let found: Vec<_> = markets.keys().filter(|k| k.to_string().starts_with(prefix)).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(*found[0], pk("FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"));

        let prefix = "DB3s";
        let found: Vec<_> = markets.keys().filter(|k| k.to_string().starts_with(prefix)).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(*found[0], pk("DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW"));
    }

    #[test]
    fn parse_setup_toml() {
        let toml = include_str!("../setup.toml");
        let cfg: Cfg = toml::from_str(toml).unwrap();

        let humidifi = cfg.humidifi.unwrap();
        assert_eq!(humidifi.swap_v1.len(), 2);
        assert!(humidifi.swap_v1.contains_key(&pk("FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH")));
        assert!(humidifi.swap_v1.contains_key(&pk("DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW")));

        let tessera = cfg.tessera.unwrap();
        assert_eq!(tessera.swap_v1.len(), 1);
        assert!(tessera.swap_v1.contains_key(&pk("FLckHLGMJy5gEoXWwcE68Nprde1D4araK4TGLw4pQq2n")));

        let goonfi = cfg.goonfi.unwrap();
        assert_eq!(goonfi.swap_v1.len(), 1);
        assert!(goonfi.swap_v1.contains_key(&pk("4uWuh9fC7rrZKrN8ZdJf69MN1e2S7FPpMqcsyY1aof6K")));

        let solfi = cfg.solfi_v2.unwrap();
        assert_eq!(solfi.swap_v1.len(), 1);
        assert!(solfi.swap_v1.contains_key(&pk("65ZHSArs5XxPseKQbB1B4r16vDxMWnCxHMzogDAqiDUc")));

        let zerofi = cfg.zerofi.unwrap();
        assert_eq!(zerofi.swap_v1.len(), 1);
        assert!(zerofi.swap_v1.contains_key(&pk("2h9hhu3gxY9kCdXEwdTHV8yPAMYVoHgKopRyG1HbDwfi")));

        let obric = cfg.obric_v2.unwrap();
        assert_eq!(obric.swap_v1.len(), 1);
        assert!(obric.swap_v1.contains_key(&pk("BWBHrYqfcjAh5dSiRwzPnY4656cApXVXmkeDmAfwBKQG")));

        let bisonfi = cfg.bisonfi.unwrap();
        assert_eq!(bisonfi.swap_v1.len(), 1);
        assert!(bisonfi.swap_v1.contains_key(&pk("51FQwjrvo8J8zXUaKyAznJ5NYpoiTCuqAqCu3HAMB9NZ")));
    }
}
