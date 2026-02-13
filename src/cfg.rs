use std::{collections::HashMap, fmt, fs, str::FromStr};

use indexmap::IndexMap;
use magnus_shared::Dex;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;

use crate::misc::Misc;

pub trait Keyed {
    fn market_key(&self) -> Pubkey;
}

/// A CLI-facing target: a DEX plus an optional market hint.
///
/// Parsed from strings like `"humidifi"` (first market) or `"humidifi_Fksf"`
/// (market whose pubkey starts with `Fksf`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PMMTarget {
    pub dex: Dex,
    pub market_hint: Option<String>,
}

impl PMMTarget {
    pub fn resolve(&self, cfg: &Cfg) -> Option<Pubkey> {
        let markets = cfg.get_accounts(&self.dex);
        match &self.market_hint {
            None => markets.first().map(|(market, _)| *market),
            Some(hint) => markets.iter().find(|(market, _)| market.to_string().starts_with(hint.as_str())).map(|(k, _)| *k),
        }
    }
}

impl FromStr for PMMTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((dex, hint)) = s.rsplit_once('_') {
            match dex.parse::<Dex>() {
                Ok(dex) => Ok(PMMTarget { dex, market_hint: Some(hint.to_string()) }),
                Err(_) => {
                    let dex = s.parse::<Dex>()?;
                    Ok(PMMTarget { dex, market_hint: None })
                }
            }
        } else {
            let dex = s.parse::<Dex>()?;
            Ok(PMMTarget { dex, market_hint: None })
        }
    }
}

impl fmt::Display for PMMTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.market_hint {
            Some(hint) => write!(f, "{}_{}", self.dex, hint),
            None => write!(f, "{}", self.dex),
        }
    }
}

fn deser_tokens<'de, D>(deserializer: D) -> Result<HashMap<String, Token>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let items: Vec<Token> = Vec::deserialize(deserializer)?;
    Ok(items.into_iter().map(|t| (t.symbol.to_uppercase(), t)).collect())
}

#[derive(Debug, Clone, Deserialize)]
pub struct Token {
    pub symbol: String,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub addr: Pubkey,
    pub dec: u8,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Cfg {
    #[serde(default, deserialize_with = "deser_tokens")]
    pub tokens: HashMap<String, Token>,

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
            ($field:expr, $ver:ident) => {
                $field.as_ref().map_or_else(Vec::new, |cfg| cfg.$ver.iter().map(|(k, v)| (*k, v.accounts())).collect())
            };
        }

        match dex {
            Dex::HumidiFi => collect_markets!(self.humidifi, swap_v1),
            Dex::Tessera => collect_markets!(self.tessera, swap_v1),
            Dex::GoonFi => collect_markets!(self.goonfi, swap_v1),
            Dex::SolfiV2 => collect_markets!(self.solfi_v2, swap_v1),
            Dex::ZeroFi => collect_markets!(self.zerofi, swap_v1),
            Dex::ObricV2 => collect_markets!(self.obric_v2, swap_v2),
            Dex::BisonFi => collect_markets!(self.bisonfi, swap_v1),
            _ => vec![],
        }
    }

    /// Returns the first market pubkey for the given DEX (for labeling / backward compat).
    pub fn get_first_market(&self, dex: &Dex) -> Option<Pubkey> {
        self.get_accounts(dex).first().map(|(k, _)| *k)
    }

    pub fn get_token(&self, symbol: &str) -> eyre::Result<&Token> {
        self.tokens
            .get(&symbol.to_uppercase())
            .ok_or_else(|| eyre::eyre!("unknown token '{symbol}' - verify the token exists in cfg/setup.toml"))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct HumidifiCfg {
    #[serde(default, deserialize_with = "Misc::deser_market")]
    pub swap_v1: IndexMap<Pubkey, HumidifiSwapV1>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HumidifiSwapV1 {
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TesseraCfg {
    #[serde(default, deserialize_with = "Misc::deser_market")]
    pub swap_v1: IndexMap<Pubkey, TesseraSwapV1>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TesseraSwapV1 {
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub quote_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub global_state: Pubkey,
}

impl TesseraSwapV1 {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.base_ta, self.quote_ta, self.global_state]
    }
}

impl Keyed for TesseraSwapV1 {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GoonfiCfg {
    #[serde(default, deserialize_with = "Misc::deser_market")]
    pub swap_v1: IndexMap<Pubkey, GoonfiSwapV1>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GoonfiSwapV1 {
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub quote_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub blacklist: Pubkey,
}

impl GoonfiSwapV1 {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.base_ta, self.quote_ta, self.blacklist]
    }
}

impl Keyed for GoonfiSwapV1 {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SolfiV2Cfg {
    #[serde(default, deserialize_with = "Misc::deser_market")]
    pub swap_v1: IndexMap<Pubkey, SolfiV2SwapV1>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SolfiV2SwapV1 {
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub quote_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub cfg: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub base_mint: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub quote_mint: Pubkey,
}

impl SolfiV2SwapV1 {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.base_ta, self.quote_ta, self.cfg, self.oracle, self.base_mint, self.quote_mint]
    }
}

impl Keyed for SolfiV2SwapV1 {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ZerofiCfg {
    #[serde(default, deserialize_with = "Misc::deser_market")]
    pub swap_v1: IndexMap<Pubkey, ZerofiSwapV1>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ZerofiSwapV1 {
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub vault_info_base: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub vault_base: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub vault_info_quote: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub vault_quote: Pubkey,
}

impl ZerofiSwapV1 {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.vault_info_base, self.vault_base, self.vault_info_quote, self.vault_quote]
    }
}

impl Keyed for ZerofiSwapV1 {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ObricV2Cfg {
    #[serde(default, deserialize_with = "Misc::deser_market")]
    pub swap_v2: IndexMap<Pubkey, ObricV2SwapV2>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ObricV2SwapV2 {
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub second_ref_oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub third_ref_oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub reserve_x: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub reserve_y: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub ref_oracle: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub x_price_feed: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub y_price_feed: Pubkey,
}

impl ObricV2SwapV2 {
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

impl Keyed for ObricV2SwapV2 {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BisonfiCfg {
    #[serde(default, deserialize_with = "Misc::deser_market")]
    pub swap_v1: IndexMap<Pubkey, BisonfiSwapV1>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BisonfiSwapV1 {
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market_base_ta: Pubkey,
    #[serde(deserialize_with = "Misc::deser_pubkey")]
    pub market_quote_ta: Pubkey,
}

impl BisonfiSwapV1 {
    pub fn accounts(&self) -> Vec<Pubkey> {
        vec![self.market, self.market_base_ta, self.market_quote_ta]
    }
}

impl Keyed for BisonfiSwapV1 {
    fn market_key(&self) -> Pubkey {
        self.market
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use solana_sdk::pubkey;

    use super::*;

    const WSOL: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
    const USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

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
    fn markets_preserve_insertion_order() {
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
        let keys: Vec<_> = cfg.humidifi.as_ref().unwrap().swap_v1.keys().collect();

        assert_eq!(*keys[0], pk("FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"));
        assert_eq!(*keys[1], pk("DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW"));

        // get_first_market must return the first declared market
        assert_eq!(cfg.get_first_market(&Dex::HumidiFi).unwrap(), pk("FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"));
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
    fn parse_tokens() {
        let toml = r#"
            [[tokens]]
            symbol = "wsol"
            addr = "So11111111111111111111111111111111111111112"
            dec = 9

            [[tokens]]
            symbol = "usdc"
            addr = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
            dec = 6

            [[tokens]]
            symbol = "usdt"
            addr = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
            dec = 6
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();

        assert_eq!(cfg.tokens.len(), 3);
        assert!(cfg.get_token("wsol").is_ok());
        assert!(cfg.get_token("usdc").is_ok());
        assert!(cfg.get_token("usdt").is_ok());
        assert!(cfg.get_token("btc").is_err());
    }

    #[test]
    fn parse_tessera() {
        let toml = r#"
            [tessera]
            [[tessera.swap-v1]]
            market = "FLckHLGMJy5gEoXWwcE68Nprde1D4araK4TGLw4pQq2n"
            base_ta = "5pVN5XZB8cYBjNLFrsBCPWkCQBan5K5Mq2dWGzwPgGJV"
            quote_ta = "9t4P5wMwfFkyn92Z7hf463qYKEZf8ERVZsGBEPNp8uJx"
            global_state = "8ekCy2jHHUbW2yeNGFWYJT9Hm9FW7SvZcZK66dSZCDiF"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let tessera = cfg.tessera.unwrap();

        assert_eq!(tessera.swap_v1.len(), 1);
        let mk = pk("FLckHLGMJy5gEoXWwcE68Nprde1D4araK4TGLw4pQq2n");
        assert!(tessera.swap_v1.contains_key(&mk));

        let entry = &tessera.swap_v1[&mk];
        assert_eq!(entry.global_state, pk("8ekCy2jHHUbW2yeNGFWYJT9Hm9FW7SvZcZK66dSZCDiF"));
    }

    #[test]
    fn parse_goonfi() {
        let toml = r#"
            [goonfi]
            [[goonfi.swap-v1]]
            market = "4uWuh9fC7rrZKrN8ZdJf69MN1e2S7FPpMqcsyY1aof6K"
            base_ta = "pKiUC9hDXv52xqU1p3BKypV9AQjAMgfZUGRnoBsdkKm"
            quote_ta = "Gsy5Zr7Vxn5KckAbduPHHGR1qzPJ4w3GSYmcinWAkhrC"
            blacklist = "7XqYD6DEGmDXooB1E8NNRWV9pWAmm1z6WYpsfjnABTUz"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let goonfi = cfg.goonfi.unwrap();

        assert_eq!(goonfi.swap_v1.len(), 1);
        let mk = pk("4uWuh9fC7rrZKrN8ZdJf69MN1e2S7FPpMqcsyY1aof6K");
        assert!(goonfi.swap_v1.contains_key(&mk));

        let entry = &goonfi.swap_v1[&mk];
        assert_eq!(entry.blacklist, pk("7XqYD6DEGmDXooB1E8NNRWV9pWAmm1z6WYpsfjnABTUz"));
    }

    #[test]
    fn parse_solfi_v2() {
        let toml = r#"
            [solfi-v2]
            [[solfi-v2.swap-v1]]
            market = "65ZHSArs5XxPseKQbB1B4r16vDxMWnCxHMzogDAqiDUc"
            base_ta = "CRo8DBwrmd97DJfAnvCv96tZPL5Mktf2NZy2ZnhDer1A"
            quote_ta = "GhFfLFSprPpfoRaWakPMmJTMJBHuz6C694jYwxy2dAic"
            cfg = "FmxXDSR9WvpJTCh738D1LEDuhMoA8geCtZgHb3isy7Dp"
            oracle = "2ny7eGyZCoeEVTkNLf5HcnJFBKkyA4p4gcrtb3b8y8ou"
            base_mint = "So11111111111111111111111111111111111111112"
            quote_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let solfi = cfg.solfi_v2.unwrap();

        assert_eq!(solfi.swap_v1.len(), 1);
        let mk = pk("65ZHSArs5XxPseKQbB1B4r16vDxMWnCxHMzogDAqiDUc");
        assert!(solfi.swap_v1.contains_key(&mk));

        let entry = &solfi.swap_v1[&mk];
        assert_eq!(entry.oracle, pk("2ny7eGyZCoeEVTkNLf5HcnJFBKkyA4p4gcrtb3b8y8ou"));
        assert_eq!(entry.base_mint, WSOL);
        assert_eq!(entry.quote_mint, USDC);
    }

    #[test]
    fn parse_zerofi() {
        let toml = r#"
            [zerofi]
            [[zerofi.swap-v1]]
            market = "2h9hhu3gxY9kCdXEwdTHV8yPAMYVoHgKopRyG1HbDwfi"
            vault_info_base = "7RHJ2WfexqUxy7SXfbNZRZDgZi3D9jtMAQp9VhfzpU8T"
            vault_base = "ERP5RTV6cWmoGrv7r9W2V5pbgDFSepc4j97qNnx1Jris"
            vault_info_quote = "Ef7zPqj4NuZHwaTczUTY9oRbxXrfZseUcKcqPaidCZ5W"
            vault_quote = "7wYJVD8iXmMQjND1fwi1hPr68QwruVVtirbotyJZXaVH"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let zerofi = cfg.zerofi.unwrap();

        assert_eq!(zerofi.swap_v1.len(), 1);
        let mk = pk("2h9hhu3gxY9kCdXEwdTHV8yPAMYVoHgKopRyG1HbDwfi");
        assert!(zerofi.swap_v1.contains_key(&mk));

        let entry = &zerofi.swap_v1[&mk];
        assert_eq!(entry.vault_base, pk("ERP5RTV6cWmoGrv7r9W2V5pbgDFSepc4j97qNnx1Jris"));
    }

    #[test]
    fn parse_obric_v2() {
        let toml = r#"
            [obric-v2]
            [[obric-v2.swap-v2]]
            market = "BWBHrYqfcjAh5dSiRwzPnY4656cApXVXmkeDmAfwBKQG"
            second_ref_oracle = "GZsNmWKbqhMYtdSkkvMdEyQF9k5mLmP7tTKYWZjcHVPE"
            third_ref_oracle = "6YawcNeZ74tRyCv4UfGydYMr7eho7vbUR6ScVffxKAb3"
            reserve_x = "C3tPQ8TRcHybnPpR8KMASUVD3PukQRRHEsLwxorJMhgm"
            reserve_y = "AAamGhyPfpQJWfZHTq944NM1cFvoVLDrQxt7HGjeRQUS"
            ref_oracle = "J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw"
            x_price_feed = "J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw"
            y_price_feed = "J4HJYz4p7TRP96WVFky3vh7XryxoFehHjoRySUTeSeXw"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let obric = cfg.obric_v2.unwrap();

        assert_eq!(obric.swap_v2.len(), 1);
        let mk = pk("BWBHrYqfcjAh5dSiRwzPnY4656cApXVXmkeDmAfwBKQG");
        assert!(obric.swap_v2.contains_key(&mk));

        let entry = &obric.swap_v2[&mk];
        assert_eq!(entry.reserve_x, pk("C3tPQ8TRcHybnPpR8KMASUVD3PukQRRHEsLwxorJMhgm"));
        assert_eq!(entry.ref_oracle, entry.x_price_feed);
    }

    #[test]
    fn parse_bisonfi() {
        let toml = r#"
            [bisonfi]
            [[bisonfi.swap-v1]]
            market = "51FQwjrvo8J8zXUaKyAznJ5NYpoiTCuqAqCu3HAMB9NZ"
            market_base_ta = "FxGiN5NkigicwrnFshZEAUH9C13yrBALmgYxA9x8sfnQ"
            market_quote_ta = "6DMF4t6Ks8yXhG8K3rrTAeNYrqNrr1DwewHvBmH3a3FX"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let bisonfi = cfg.bisonfi.unwrap();

        assert_eq!(bisonfi.swap_v1.len(), 1);
        let mk = pk("51FQwjrvo8J8zXUaKyAznJ5NYpoiTCuqAqCu3HAMB9NZ");
        assert!(bisonfi.swap_v1.contains_key(&mk));

        let entry = &bisonfi.swap_v1[&mk];
        assert_eq!(entry.market_base_ta, pk("FxGiN5NkigicwrnFshZEAUH9C13yrBALmgYxA9x8sfnQ"));
    }

    #[test]
    fn get_accounts_returns_market_keys_and_pubkeys() {
        let toml = r#"
            [humidifi]
            [[humidifi.swap-v1]]
            market = "FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"
            base_ta = "C3FzbX9n1YD2dow2dCmEv5uNyyf22Gb3TLAEqGBhw5fY"
            quote_ta = "3RWFAQBRkNGq7CMGcTLK3kXDgFTe9jgMeFYqk8nHwcWh"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();
        let accs = cfg.get_accounts(&Dex::HumidiFi);

        assert_eq!(accs.len(), 1);
        assert_eq!(accs[0].0, pk("FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"));
        assert_eq!(accs[0].1.len(), 3);
    }

    #[test]
    fn get_first_market_returns_none_when_unconfigured() {
        let cfg: Cfg = toml::from_str("").unwrap();

        assert!(cfg.get_first_market(&Dex::HumidiFi).is_none());
        assert!(cfg.get_first_market(&Dex::Tessera).is_none());
    }

    #[test]
    fn empty_cfg_has_no_pmms() {
        let cfg: Cfg = toml::from_str("").unwrap();

        assert!(cfg.humidifi.is_none());
        assert!(cfg.tessera.is_none());
        assert!(cfg.goonfi.is_none());
        assert!(cfg.solfi_v2.is_none());
        assert!(cfg.zerofi.is_none());
        assert!(cfg.obric_v2.is_none());
        assert!(cfg.bisonfi.is_none());
    }

    #[test]
    fn multi_pmm_cfg() {
        let toml = r#"
            [[tokens]]
            symbol = "wsol"
            addr = "So11111111111111111111111111111111111111112"
            dec = 9

            [humidifi]
            [[humidifi.swap-v1]]
            market = "FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH"
            base_ta = "C3FzbX9n1YD2dow2dCmEv5uNyyf22Gb3TLAEqGBhw5fY"
            quote_ta = "3RWFAQBRkNGq7CMGcTLK3kXDgFTe9jgMeFYqk8nHwcWh"

            [[humidifi.swap-v1]]
            market = "DB3sUCP2H4icbeKmK6yb6nUxU5ogbcRHtGuq7W2RoRwW"
            base_ta = "8BrVfsvzb1DZqCactbYWoKSv24AfsLBuXJqzpzYCwznF"
            quote_ta = "HsQcHFFNUVTp3MWrXYbuZchBNd4Pwk8636bKzLvpfYNR"

            [tessera]
            [[tessera.swap-v1]]
            market = "FLckHLGMJy5gEoXWwcE68Nprde1D4araK4TGLw4pQq2n"
            base_ta = "5pVN5XZB8cYBjNLFrsBCPWkCQBan5K5Mq2dWGzwPgGJV"
            quote_ta = "9t4P5wMwfFkyn92Z7hf463qYKEZf8ERVZsGBEPNp8uJx"
            global_state = "8ekCy2jHHUbW2yeNGFWYJT9Hm9FW7SvZcZK66dSZCDiF"

            [bisonfi]
            [[bisonfi.swap-v1]]
            market = "51FQwjrvo8J8zXUaKyAznJ5NYpoiTCuqAqCu3HAMB9NZ"
            market_base_ta = "FxGiN5NkigicwrnFshZEAUH9C13yrBALmgYxA9x8sfnQ"
            market_quote_ta = "6DMF4t6Ks8yXhG8K3rrTAeNYrqNrr1DwewHvBmH3a3FX"
        "#;

        let cfg: Cfg = toml::from_str(toml).unwrap();

        assert_eq!(cfg.tokens.len(), 1);
        assert!(cfg.get_token("wsol").is_ok());

        assert_eq!(cfg.humidifi.as_ref().unwrap().swap_v1.len(), 2);
        assert_eq!(cfg.tessera.as_ref().unwrap().swap_v1.len(), 1);
        assert_eq!(cfg.bisonfi.as_ref().unwrap().swap_v1.len(), 1);
        assert!(cfg.goonfi.is_none());
        assert!(cfg.solfi_v2.is_none());
        assert!(cfg.zerofi.is_none());
        assert!(cfg.obric_v2.is_none());

        assert!(cfg.get_first_market(&Dex::HumidiFi).is_some());
        assert!(cfg.get_first_market(&Dex::Tessera).is_some());
        assert!(cfg.get_first_market(&Dex::BisonFi).is_some());
    }

    mod tokens {
        use super::*;

        fn cfg_with_tokens() -> Cfg {
            toml::from_str(
                r#"
                [[tokens]]
                symbol = "wsol"
                addr = "So11111111111111111111111111111111111111112"
                dec = 9

                [[tokens]]
                symbol = "usdc"
                addr = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
                dec = 6
                "#,
            )
            .unwrap()
        }

        #[test]
        fn test_token_deserializes_from_toml() {
            let cfg = cfg_with_tokens();
            let wsol = cfg.get_token("wsol").unwrap();

            assert_eq!(wsol.symbol, "wsol");
            assert_eq!(wsol.addr, WSOL);
            assert_eq!(wsol.dec, 9);
        }

        #[test]
        fn test_get_token_case_insensitive() {
            let cfg = cfg_with_tokens();

            assert!(cfg.get_token("wsol").is_ok());
            assert!(cfg.get_token("WSOL").is_ok());
            assert!(cfg.get_token("Wsol").is_ok());
            assert_eq!(cfg.get_token("wsol").unwrap().addr, WSOL);
        }

        #[test]
        fn test_get_token_unknown_symbol_errors() {
            let cfg = cfg_with_tokens();

            assert!(cfg.get_token("btc").is_err());
        }

        #[test]
        fn test_get_token_returns_correct_entry() {
            let cfg = cfg_with_tokens();

            let usdc = cfg.get_token("usdc").unwrap();
            assert_eq!(usdc.addr, USDC);
            assert_eq!(usdc.dec, 6);
        }

        #[test]
        fn test_empty_cfg_has_no_tokens() {
            let cfg: Cfg = toml::from_str("").unwrap();

            assert!(cfg.get_token("wsol").is_err());
        }
    }
}
