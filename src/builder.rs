use magnus_router_client::{
    instructions::{RouteV2Builder, Swap2Builder, SwapBuilder, SwapRouteV2Builder, SwapV3WithCpiEventBuilder},
    types::SwapArgs,
};
use magnus_shared::Dex;
use solana_sdk::{instruction::Instruction, message::AccountMeta, pubkey::Pubkey};

use crate::{
    Aggregator,
    cfg::{Cfg, Swap},
    misc::Misc,
};

/// A helper struct to construct swap instructions with the required accounts
/// for different Prop AMMs.
///
/// As it currently stands, all swaps pass through the Magnus Router program,
/// which in turn calls the respective Prop AMM program. The correct instruction
/// builder is selected based on the target aggregator, ensuring the right
/// discriminator is used (e.g. `route_v2` for Jupiter, `swap2` for DFlow, etc.).
///
/// The order of the remaining_accounts matters.
pub struct ConstructSwap {
    pub cfg: Cfg,
    pub remaining_accounts: Vec<AccountMeta>,
    pub payer: Pubkey,
    pub src_ta: Pubkey,
    pub dst_ta: Pubkey,
    pub src_mint: Pubkey,
    pub dst_mint: Pubkey,
}

impl ConstructSwap {
    /// Builds the final instruction using the appropriate builder for the given aggregator.
    ///
    /// Each aggregator maps to a specific router instruction:
    /// - `None` (magnus) -> `swap`
    /// - `Jupiter` -> `route_v2`
    /// - `DFlow` -> `swap2`
    /// - `Titan` -> `swap_route_v2`
    /// - `OkxLabs` -> `swap_v3_with_cpi_event`
    pub fn instruction(&self, spoof: Option<Aggregator>, data: SwapArgs, order_id: u64) -> Instruction {
        macro_rules! build_ix {
            ($builder:ident) => {{
                let mut builder = $builder::new();
                builder
                    .payer(self.payer)
                    .source_token_account(self.src_ta)
                    .destination_token_account(self.dst_ta)
                    .source_mint(self.src_mint)
                    .destination_mint(self.dst_mint)
                    .data(data)
                    .order_id(order_id)
                    .add_remaining_accounts(&self.remaining_accounts);
                let mut ix = builder.instruction();
                if let Some(aggr) = spoof {
                    ix.program_id = aggr.program_id();
                }
                ix
            }};
        }

        match spoof {
            None => build_ix!(SwapBuilder),
            Some(Aggregator::Jupiter) => build_ix!(RouteV2Builder),
            Some(Aggregator::DFlow) => build_ix!(Swap2Builder),
            Some(Aggregator::Titan) => build_ix!(SwapRouteV2Builder),
            Some(Aggregator::OkxLabs) => build_ix!(SwapV3WithCpiEventBuilder),
        }
    }

    /// Attaches the required remaining accounts for the specified PMM to the swap instruction.
    ///
    /// Each Prop AMM program expects a specific set of accounts in a precise order as
    /// "remaining accounts" on the swap instruction. This method dispatches to the
    /// appropriate PMM-specific attachment function based on the DEX type.
    pub fn attach_pmm_accs(&mut self, pmm: &Dex, market: &Pubkey) -> &mut Self {
        match pmm {
            Dex::HumidiFi => self.attach_humidifi_accs(market),
            Dex::HumidiFiSwapV2 | Dex::HumidiFiSwapV3 => self.attach_humidifi_swap_v2v3_accs(pmm, market),
            Dex::SolfiV2 => self.attach_solfiv2_accs(market),
            Dex::ZeroFi => self.attach_zerofi_accs(market),
            Dex::ObricV2 => self.attach_obric_v2_accs(market),
            Dex::Tessera => self.attach_tessera_accs(market),
            Dex::GoonFi => self.attach_goonfi_accs(market),
            Dex::BisonFi => self.attach_bisonfi_accs(market),
            _ => {
                unimplemented!()
            }
        };

        self
    }

    /// Attaches the required remaining accounts for multiple PMMs to the swap instruction.
    pub fn attach_pmms_accs(&mut self, pmms: &[(Dex, Pubkey)]) -> &mut Self {
        pmms.iter().for_each(|(pmm, market)| {
            self.attach_pmm_accs(pmm, market);
        });

        self
    }

    pub fn attach_solfiv2_accs(&mut self, market: &Pubkey) {
        let cfg = self
            .cfg
            .solfi_v2
            .as_ref()
            .and_then(|c| c.swap_v1.get(market))
            .unwrap_or_else(|| panic!("SolFiV2 market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_solfi_v2::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, None, None));
        self.remaining_accounts.extend(accs);
    }

    pub fn attach_humidifi_accs(&mut self, market: &Pubkey) {
        let cfg = self
            .cfg
            .humidifi
            .as_ref()
            .and_then(|c| c.swap_v1.get(market))
            .unwrap_or_else(|| panic!("HumidiFi market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_humidifi::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, None, None));
        accs.push(AccountMeta::new_readonly(Misc::create_humidifi_param(1500), false));
        self.remaining_accounts.extend(accs);
    }

    pub fn attach_humidifi_swap_v2v3_accs(&mut self, pmm: &Dex, market: &Pubkey) {
        let humidifi = self.cfg.humidifi.as_ref().unwrap_or_else(|| panic!("HumidiFi not configured"));
        let cfg = match pmm {
            Dex::HumidiFiSwapV2 => humidifi.swap_v2.get(market),
            Dex::HumidiFiSwapV3 => humidifi.swap_v3.get(market),
            _ => unreachable!(),
        }
        .unwrap_or_else(|| panic!("{pmm} market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_humidifi::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, None, None));
        accs.push(AccountMeta::new_readonly(Misc::create_humidifi_param(1500), false));
        self.remaining_accounts.extend(accs);
    }

    pub fn attach_zerofi_accs(&mut self, market: &Pubkey) {
        let cfg =
            self.cfg.zerofi.as_ref().and_then(|c| c.swap_v1.get(market)).unwrap_or_else(|| panic!("ZeroFi market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_zerofi::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, None, None));
        self.remaining_accounts.extend(accs);
    }

    pub fn attach_obric_v2_accs(&mut self, market: &Pubkey) {
        let cfg = self
            .cfg
            .obric_v2
            .as_ref()
            .and_then(|c| c.swap_v2.get(market))
            .unwrap_or_else(|| panic!("ObricV2 market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_obric_v2::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, None, None));
        self.remaining_accounts.extend(accs);
    }

    pub fn attach_tessera_accs(&mut self, market: &Pubkey) {
        let cfg = self
            .cfg
            .tessera
            .as_ref()
            .and_then(|c| c.swap_v1.get(market))
            .unwrap_or_else(|| panic!("Tessera market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_tessera::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, Some(self.src_mint), Some(self.dst_mint)));
        self.remaining_accounts.extend(accs);
    }

    pub fn attach_goonfi_accs(&mut self, market: &Pubkey) {
        let cfg =
            self.cfg.goonfi.as_ref().and_then(|c| c.swap_v1.get(market)).unwrap_or_else(|| panic!("GoonFi market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_goonfi::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, None, None));
        accs.push(AccountMeta::new_readonly(Pubkey::new_from_array([0u8; 32]), false));
        self.remaining_accounts.extend(accs);
    }

    pub fn attach_bisonfi_accs(&mut self, market: &Pubkey) {
        let cfg = self
            .cfg
            .bisonfi
            .as_ref()
            .and_then(|c| c.swap_v1.get(market))
            .unwrap_or_else(|| panic!("BisonFi market {market} not configured"));

        let mut accs = vec![AccountMeta::new_readonly(Pubkey::new_from_array(magnus_shared::pmm_bisonfi::id().to_bytes()), false)];
        accs.extend(cfg.swap_accounts(self.payer, self.src_ta, self.dst_ta, None, None));
        self.remaining_accounts.extend(accs);
    }
}
