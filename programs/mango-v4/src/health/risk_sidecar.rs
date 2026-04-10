use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::Hasher;
use anchor_lang::ToAccountInfo;
use fixed::types::I80F48;

use crate::error::{IsAnchorErrorWithCode, MangoError};
use crate::serum3_cpi::OpenOrdersAmounts;
use crate::state::{
    MangoAccountRef, MangoAccountRefMut, OpenbookV2MarketIndex, PerpMarket, RiskSidecar,
    Serum3MarketIndex, TokenIndex,
};

use super::{
    new_fixed_order_account_retriever_with_optional_banks, new_health_cache, new_health_cache_skipping_missing_banks_and_bad_oracles,
    AccountRetriever, HealthCache, HealthType, PerpInfo, Prices, SpotInfo, SpotMarketIndex,
    TokenInfo,
};

/// Exact rebuild snapshot for the future incremental risk sidecar.
///
/// This keeps the current cross-margin semantics intact by materializing the
/// same token/spot/perp inputs that a fresh `HealthCache` would use, together
/// with the health totals they produced. The first implementation goal is exact
/// rebuild parity; incremental maintenance can layer on top of this format.
#[derive(Clone, Debug)]
pub struct ExactRiskSidecarMeta {
    pub group: Pubkey,
    pub mango_account: Pubkey,
    pub account_sequence_number: u8,
    pub oracle_slot: u64,
    pub token_count: u16,
    pub spot_count: u16,
    pub perp_count: u16,
}

#[derive(Clone, Debug)]
pub struct ExactRiskSidecarSnapshot {
    pub meta: ExactRiskSidecarMeta,
    pub token_infos: Vec<TokenInfo>,
    pub spot_infos: Vec<SpotInfo>,
    pub perp_infos: Vec<PerpInfo>,
    pub being_liquidated: bool,
    pub init_health: I80F48,
    pub maint_health: I80F48,
    pub liquidation_end_health: I80F48,
}

/// Shared exact-semantics pre/post health session for future sidecar-backed paths.
///
/// This keeps the hot path on a single health-cache build while allowing callers
/// to materialize an exact sidecar snapshot from the same session later, without
/// rebuilding health a second time.
pub struct ExactRiskSidecarSession {
    group: Pubkey,
    mango_account: Pubkey,
    oracle_slot: u64,
    health_cache: HealthCache,
    pre_init_health: I80F48,
}

const SERIALIZED_PRICES_BYTES: usize = 16 * 2;
const SERIALIZED_TOKEN_INFO_BYTES: usize = 2 + 16 * 6 + SERIALIZED_PRICES_BYTES + 16 + 1;
const SERIALIZED_SPOT_INFO_BYTES: usize = 16 * 4 + 2 + 2 + 1 + 2 + 1;
const SERIALIZED_PERP_INFO_BYTES: usize = 2 + 2 + 16 * 8 + 8 * 4 + 16 + SERIALIZED_PRICES_BYTES + 1 + 1;
const SERIALIZED_SNAPSHOT_PREFIX_BYTES: usize = 2 + 2 + 2 + 1;

impl ExactRiskSidecarSnapshot {
    pub fn rebuild(
        group: Pubkey,
        mango_account: Pubkey,
        account: &MangoAccountRef,
        health_cache: &HealthCache,
        oracle_slot: u64,
    ) -> Self {
        Self {
            meta: ExactRiskSidecarMeta {
                group,
                mango_account,
                account_sequence_number: account.fixed.sequence_number,
                oracle_slot,
                token_count: health_cache.token_infos.len() as u16,
                spot_count: health_cache.spot_infos.len() as u16,
                perp_count: health_cache.perp_infos.len() as u16,
            },
            token_infos: health_cache.token_infos.clone(),
            spot_infos: health_cache.spot_infos.clone(),
            perp_infos: health_cache.perp_infos.clone(),
            being_liquidated: health_cache.being_liquidated,
            init_health: health_cache.health(HealthType::Init),
            maint_health: health_cache.health(HealthType::Maint),
            liquidation_end_health: health_cache.health(HealthType::LiquidationEnd),
        }
    }

    pub fn as_health_cache(&self) -> HealthCache {
        HealthCache {
            token_infos: self.token_infos.clone(),
            spot_infos: self.spot_infos.clone(),
            perp_infos: self.perp_infos.clone(),
            being_liquidated: self.being_liquidated,
        }
    }

    pub fn matches_health_cache(&self) -> bool {
        let health_cache = self.as_health_cache();
        self.init_health == health_cache.health(HealthType::Init)
            && self.maint_health == health_cache.health(HealthType::Maint)
            && self.liquidation_end_health == health_cache.health(HealthType::LiquidationEnd)
    }

    pub fn from_health_cache(
        mango_account: Pubkey,
        account: &MangoAccountRef,
        health_cache: &HealthCache,
        oracle_slot: u64,
    ) -> Self {
        Self::rebuild(
            account.fixed.group,
            mango_account,
            account,
            health_cache,
            oracle_slot,
        )
    }

    pub fn from_retriever(
        mango_account: Pubkey,
        account: &MangoAccountRef,
        retriever: &impl AccountRetriever,
        now_ts: u64,
        oracle_slot: u64,
    ) -> Result<Self> {
        let health_cache = new_health_cache(account, retriever, now_ts)?;
        Ok(Self::from_health_cache(
            mango_account,
            account,
            &health_cache,
            oracle_slot,
        ))
    }

    pub fn from_retriever_skipping_missing_banks_and_bad_oracles(
        mango_account: Pubkey,
        account: &MangoAccountRef,
        retriever: &impl AccountRetriever,
        now_ts: u64,
        oracle_slot: u64,
    ) -> Result<Self> {
        let health_cache =
            new_health_cache_skipping_missing_banks_and_bad_oracles(account, retriever, now_ts)?;
        Ok(Self::from_health_cache(
            mango_account,
            account,
            &health_cache,
            oracle_slot,
        ))
    }

    pub fn from_fixed_accounts<'info>(
        mango_account: Pubkey,
        account: &MangoAccountRef,
        ais: &[AccountInfo<'info>],
        slot: u64,
        now_ts: u64,
    ) -> Result<Self> {
        let retriever = new_fixed_order_account_retriever_with_optional_banks(ais, account, slot)?;
        Self::from_retriever_skipping_missing_banks_and_bad_oracles(
            mango_account,
            account,
            &retriever,
            now_ts,
            slot,
        )
    }

    pub fn encoded_len(&self) -> usize {
        SERIALIZED_SNAPSHOT_PREFIX_BYTES
            + self.token_infos.len() * SERIALIZED_TOKEN_INFO_BYTES
            + self.spot_infos.len() * SERIALIZED_SPOT_INFO_BYTES
            + self.perp_infos.len() * SERIALIZED_PERP_INFO_BYTES
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.encoded_len());
        push_u16(&mut out, self.token_infos.len())?;
        push_u16(&mut out, self.spot_infos.len())?;
        push_u16(&mut out, self.perp_infos.len())?;
        push_bool(&mut out, self.being_liquidated);

        for token in &self.token_infos {
            push_u16(&mut out, usize::from(token.token_index))?;
            push_i128(&mut out, token.maint_asset_weight.to_bits());
            push_i128(&mut out, token.init_asset_weight.to_bits());
            push_i128(&mut out, token.init_scaled_asset_weight.to_bits());
            push_i128(&mut out, token.maint_liab_weight.to_bits());
            push_i128(&mut out, token.init_liab_weight.to_bits());
            push_i128(&mut out, token.init_scaled_liab_weight.to_bits());
            encode_prices(&mut out, &token.prices);
            push_i128(&mut out, token.balance_spot.to_bits());
            push_bool(&mut out, token.allow_asset_liquidation);
        }

        for spot in &self.spot_infos {
            push_i128(&mut out, spot.reserved_base.to_bits());
            push_i128(&mut out, spot.reserved_quote.to_bits());
            push_i128(&mut out, spot.reserved_base_as_quote_lowest_ask.to_bits());
            push_i128(&mut out, spot.reserved_quote_as_base_highest_bid.to_bits());
            push_u16(&mut out, spot.base_info_index)?;
            push_u16(&mut out, spot.quote_info_index)?;
            match spot.spot_market_index {
                SpotMarketIndex::Serum3(index) => {
                    push_u8(&mut out, 0);
                    push_u16(&mut out, usize::from(index))?;
                }
                SpotMarketIndex::OpenbookV2(index) => {
                    push_u8(&mut out, 1);
                    push_u16(&mut out, usize::from(index))?;
                }
            }
            push_bool(&mut out, spot.has_zero_funds);
        }

        for perp in &self.perp_infos {
            push_u16(&mut out, usize::from(perp.perp_market_index))?;
            push_u16(&mut out, usize::from(perp.settle_token_index))?;
            push_i128(&mut out, perp.maint_base_asset_weight.to_bits());
            push_i128(&mut out, perp.init_base_asset_weight.to_bits());
            push_i128(&mut out, perp.maint_base_liab_weight.to_bits());
            push_i128(&mut out, perp.init_base_liab_weight.to_bits());
            push_i128(&mut out, perp.maint_overall_asset_weight.to_bits());
            push_i128(&mut out, perp.init_overall_asset_weight.to_bits());
            push_i64(&mut out, perp.base_lot_size);
            push_i64(&mut out, perp.base_lots);
            push_i64(&mut out, perp.bids_base_lots);
            push_i64(&mut out, perp.asks_base_lots);
            push_i128(&mut out, perp.quote.to_bits());
            encode_prices(&mut out, &perp.base_prices);
            push_bool(&mut out, perp.has_open_orders);
            push_bool(&mut out, perp.has_open_fills);
        }

        Ok(out)
    }

    pub fn decode(
        mut meta: ExactRiskSidecarMeta,
        init_health: I80F48,
        maint_health: I80F48,
        liquidation_end_health: I80F48,
        bytes: &[u8],
    ) -> Result<Self> {
        let mut cursor = bytes;
        let token_count = read_u16(&mut cursor)? as usize;
        let spot_count = read_u16(&mut cursor)? as usize;
        let perp_count = read_u16(&mut cursor)? as usize;
        let decoded_being_liquidated = read_bool(&mut cursor)?;

        let mut token_infos = Vec::with_capacity(token_count);
        for _ in 0..token_count {
            token_infos.push(TokenInfo {
                token_index: read_u16(&mut cursor)?,
                maint_asset_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                init_asset_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                init_scaled_asset_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                maint_liab_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                init_liab_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                init_scaled_liab_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                prices: decode_prices(&mut cursor)?,
                balance_spot: I80F48::from_bits(read_i128(&mut cursor)?),
                allow_asset_liquidation: read_bool(&mut cursor)?,
            });
        }

        let mut spot_infos = Vec::with_capacity(spot_count);
        for _ in 0..spot_count {
            let reserved_base = I80F48::from_bits(read_i128(&mut cursor)?);
            let reserved_quote = I80F48::from_bits(read_i128(&mut cursor)?);
            let reserved_base_as_quote_lowest_ask = I80F48::from_bits(read_i128(&mut cursor)?);
            let reserved_quote_as_base_highest_bid = I80F48::from_bits(read_i128(&mut cursor)?);
            let base_info_index = read_u16(&mut cursor)? as usize;
            let quote_info_index = read_u16(&mut cursor)? as usize;
            let spot_market_kind = read_u8(&mut cursor)?;
            let spot_market_raw_index = read_u16(&mut cursor)?;
            let spot_market_index = match spot_market_kind {
                0 => SpotMarketIndex::Serum3(spot_market_raw_index as Serum3MarketIndex),
                1 => SpotMarketIndex::OpenbookV2(spot_market_raw_index as OpenbookV2MarketIndex),
                _ => return err!(MangoError::RiskSidecarSnapshotDecodeFailed),
            };
            spot_infos.push(SpotInfo {
                reserved_base,
                reserved_quote,
                reserved_base_as_quote_lowest_ask,
                reserved_quote_as_base_highest_bid,
                base_info_index,
                quote_info_index,
                spot_market_index,
                has_zero_funds: read_bool(&mut cursor)?,
            });
        }

        let mut perp_infos = Vec::with_capacity(perp_count);
        for _ in 0..perp_count {
            perp_infos.push(PerpInfo {
                perp_market_index: read_u16(&mut cursor)?,
                settle_token_index: read_u16(&mut cursor)?,
                maint_base_asset_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                init_base_asset_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                maint_base_liab_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                init_base_liab_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                maint_overall_asset_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                init_overall_asset_weight: I80F48::from_bits(read_i128(&mut cursor)?),
                base_lot_size: read_i64(&mut cursor)?,
                base_lots: read_i64(&mut cursor)?,
                bids_base_lots: read_i64(&mut cursor)?,
                asks_base_lots: read_i64(&mut cursor)?,
                quote: I80F48::from_bits(read_i128(&mut cursor)?),
                base_prices: decode_prices(&mut cursor)?,
                has_open_orders: read_bool(&mut cursor)?,
                has_open_fills: read_bool(&mut cursor)?,
            });
        }

        require!(cursor.is_empty(), MangoError::RiskSidecarSnapshotDecodeFailed);
        meta.token_count = token_infos.len() as u16;
        meta.spot_count = spot_infos.len() as u16;
        meta.perp_count = perp_infos.len() as u16;

        Ok(Self {
            meta,
            token_infos,
            spot_infos,
            perp_infos,
            being_liquidated: decoded_being_liquidated,
            init_health,
            maint_health,
            liquidation_end_health,
        })
    }
}

impl ExactRiskSidecarSession {
    fn from_health_cache(
        mango_account: Pubkey,
        account: &mut MangoAccountRefMut,
        health_cache: HealthCache,
        oracle_slot: u64,
    ) -> Result<Self> {
        let pre_init_health = account.check_health_pre(&health_cache)?;
        Ok(Self {
            group: account.fixed.group,
            mango_account,
            oracle_slot,
            health_cache,
            pre_init_health,
        })
    }

    pub fn prepare_from_snapshot(
        mango_account: Pubkey,
        account: &mut MangoAccountRefMut,
        snapshot: &ExactRiskSidecarSnapshot,
    ) -> Result<Self> {
        Self::from_health_cache(mango_account, account, snapshot.as_health_cache(), snapshot.meta.oracle_slot)
    }

    pub fn prepare_from_retriever(
        mango_account: Pubkey,
        account: &mut MangoAccountRefMut,
        retriever: &impl AccountRetriever,
        now_ts: u64,
        oracle_slot: u64,
    ) -> Result<Self> {
        let health_cache = new_health_cache(&account.borrow(), retriever, now_ts)?;
        Self::from_health_cache(mango_account, account, health_cache, oracle_slot)
    }

    pub fn prepare_from_retriever_skipping_missing_banks_and_bad_oracles(
        mango_account: Pubkey,
        account: &mut MangoAccountRefMut,
        retriever: &impl AccountRetriever,
        now_ts: u64,
        oracle_slot: u64,
    ) -> Result<Self> {
        let health_cache =
            new_health_cache_skipping_missing_banks_and_bad_oracles(&account.borrow(), retriever, now_ts)?;
        Self::from_health_cache(mango_account, account, health_cache, oracle_slot)
    }

    pub fn prepare_from_fixed_accounts<'info>(
        mango_account: Pubkey,
        account: &mut MangoAccountRefMut,
        ais: &[AccountInfo<'info>],
        slot: u64,
        now_ts: u64,
    ) -> Result<Self> {
        let retriever =
            new_fixed_order_account_retriever_with_optional_banks(ais, &account.borrow(), slot)?;
        Self::prepare_from_retriever_skipping_missing_banks_and_bad_oracles(
            mango_account,
            account,
            &retriever,
            now_ts,
            slot,
        )
    }

    pub fn require_token_info(&self, token_index: TokenIndex) -> Result<()> {
        self.health_cache.token_info_index(token_index)?;
        Ok(())
    }

    pub fn recompute_perp_and_check_post(
        &mut self,
        account: &mut MangoAccountRefMut,
        perp_market: &PerpMarket,
    ) -> Result<I80F48> {
        let perp_position = account.perp_position(perp_market.perp_market_index)?;
        self.health_cache
            .recompute_perp_info(perp_position, perp_market)?;
        account.check_health_post(&self.health_cache, self.pre_init_health)
    }

    pub fn snapshot(&self, account: &MangoAccountRef) -> ExactRiskSidecarSnapshot {
        self.health_cache.exact_risk_sidecar_snapshot(
            self.group,
            self.mango_account,
            account,
            self.oracle_slot,
        )
    }

    pub fn into_snapshot(self, account: &MangoAccountRef) -> ExactRiskSidecarSnapshot {
        self.snapshot(account)
    }
}

pub fn split_optional_risk_sidecar_account<'a, 'info>(
    group: Pubkey,
    mango_account: Pubkey,
    ais: &'a [AccountInfo<'info>],
) -> (Option<&'a AccountInfo<'info>>, &'a [AccountInfo<'info>]) {
    let Some(first) = ais.first() else {
        return (None, ais);
    };
    let (expected_key, _) = RiskSidecar::pda(&group, &mango_account);
    if first.key == &expected_key {
        (Some(first), &ais[1..])
    } else {
        (None, ais)
    }
}

pub fn risk_sidecar_account_state_hash(account: &MangoAccountRef) -> Result<[u8; 32]> {
    let mut hasher = Hasher::default();
    hasher.hash(b"risk-sidecar-account-v2");
    hash_pubkey(&mut hasher, &{ account.fixed.group });
    hash_u8(&mut hasher, { account.fixed.sequence_number });
    hash_bool(&mut hasher, account.fixed.being_liquidated());

    hasher.hash(b"tokens");
    let mut token_count = 0u16;
    for token_position in account.active_token_positions() {
        token_count = token_count.saturating_add(1);
        hash_u16(&mut hasher, { token_position.token_index });
        hash_i128(&mut hasher, { token_position.indexed_position }.to_bits());
    }
    hash_u16(&mut hasher, token_count);

    hasher.hash(b"serum3");
    let mut serum3_count = 0u16;
    for serum3_orders in account.active_serum3_orders() {
        serum3_count = serum3_count.saturating_add(1);
        hash_pubkey(&mut hasher, &{ serum3_orders.open_orders });
        hash_u16(&mut hasher, { serum3_orders.market_index });
        hash_u16(&mut hasher, { serum3_orders.base_token_index });
        hash_u16(&mut hasher, { serum3_orders.quote_token_index });
        hash_f64(&mut hasher, { serum3_orders.highest_placed_bid_inv });
        hash_f64(&mut hasher, { serum3_orders.lowest_placed_ask });
    }
    hash_u16(&mut hasher, serum3_count);

    hasher.hash(b"openbook-v2");
    let mut openbook_count = 0u16;
    for openbook_orders in account.active_openbook_v2_orders() {
        openbook_count = openbook_count.saturating_add(1);
        hash_pubkey(&mut hasher, &{ openbook_orders.open_orders });
        hash_u16(&mut hasher, { openbook_orders.market_index });
        hash_u16(&mut hasher, { openbook_orders.base_token_index });
        hash_u16(&mut hasher, { openbook_orders.quote_token_index });
        hash_f64(&mut hasher, { openbook_orders.highest_placed_bid_inv });
        hash_f64(&mut hasher, { openbook_orders.lowest_placed_ask });
        hash_i64(&mut hasher, { openbook_orders.base_lot_size });
        hash_i64(&mut hasher, { openbook_orders.quote_lot_size });
    }
    hash_u16(&mut hasher, openbook_count);

    hasher.hash(b"perps");
    let mut perp_count = 0u16;
    for perp_position in account.active_perp_positions() {
        perp_count = perp_count.saturating_add(1);
        hash_u16(&mut hasher, { perp_position.market_index });
        hash_i64(&mut hasher, perp_position.base_position_lots());
        hash_i128(&mut hasher, perp_position.quote_position_native().to_bits());
        hash_i128(&mut hasher, { perp_position.long_settled_funding }.to_bits());
        hash_i128(&mut hasher, { perp_position.short_settled_funding }.to_bits());
        hash_i64(&mut hasher, { perp_position.bids_base_lots });
        hash_i64(&mut hasher, { perp_position.asks_base_lots });
        hash_i64(&mut hasher, { perp_position.taker_base_lots });
        hash_i64(&mut hasher, { perp_position.taker_quote_lots });
    }
    hash_u16(&mut hasher, perp_count);

    Ok(hasher.result().to_bytes())
}

pub fn risk_sidecar_health_accounts_state_hash<'info>(
    account: &MangoAccountRef,
    ais: &[AccountInfo<'info>],
    slot: u64,
    now_ts: u64,
) -> Result<[u8; 32]> {
    let retriever = new_fixed_order_account_retriever_with_optional_banks(ais, account, slot)?;
    let available_banks = retriever.available_banks()?;

    let mut hasher = Hasher::default();
    hasher.hash(b"risk-sidecar-health-v2");

    let mut included_token_indices = Vec::with_capacity(account.active_token_positions().count());

    hasher.hash(b"tokens");
    let mut token_count = 0u16;
    for (i, position) in account.active_token_positions().enumerate() {
        let token_index = { position.token_index };
        let indexed_position = { position.indexed_position };

        if !available_banks.contains(&token_index) {
            require!(indexed_position >= I80F48::ZERO, MangoError::InvalidBank);
            continue;
        }

        let bank_oracle_result = retriever.bank_and_oracle(&account.fixed.group, i, token_index);
        if bank_oracle_result.is_oracle_error() && indexed_position >= I80F48::ZERO {
            continue;
        }
        let (bank, oracle_price) = bank_oracle_result?;
        let native = position.native(bank);
        let stable_price = bank.stable_price();
        let liab_price = oracle_price.max(stable_price);
        let (maint_asset_weight, maint_liab_weight) = bank.maint_weights(now_ts);

        token_count = token_count.saturating_add(1);
        included_token_indices.push(bank.token_index);
        hash_u16(&mut hasher, bank.token_index);
        hash_i128(&mut hasher, native.to_bits());
        hash_i128(&mut hasher, maint_asset_weight.to_bits());
        hash_i128(&mut hasher, bank.init_asset_weight.to_bits());
        hash_i128(&mut hasher, bank.scaled_init_asset_weight(liab_price).to_bits());
        hash_i128(&mut hasher, maint_liab_weight.to_bits());
        hash_i128(&mut hasher, bank.init_liab_weight.to_bits());
        hash_i128(&mut hasher, bank.scaled_init_liab_weight(liab_price).to_bits());
        hash_i128(&mut hasher, oracle_price.to_bits());
        hash_i128(&mut hasher, stable_price.to_bits());
        hash_bool(&mut hasher, bank.allows_asset_liquidation());
    }
    hash_u16(&mut hasher, token_count);

    hasher.hash(b"spots");
    let mut spot_count = 0u16;
    for (i, serum_orders) in account.active_serum3_orders().enumerate() {
        let base_token_index = { serum_orders.base_token_index };
        let quote_token_index = { serum_orders.quote_token_index };
        if !included_token_indices.contains(&base_token_index)
            || !included_token_indices.contains(&quote_token_index)
        {
            continue;
        }
        let open_orders = retriever.serum_oo(i, &{ serum_orders.open_orders })?;
        spot_count = spot_count.saturating_add(1);
        hash_u8(&mut hasher, 0);
        hash_u16(&mut hasher, { serum_orders.market_index });
        hash_u64(&mut hasher, open_orders.native_base_free());
        hash_u64(&mut hasher, open_orders.native_quote_free());
        hash_u64(&mut hasher, open_orders.native_base_reserved());
        hash_u64(&mut hasher, open_orders.native_quote_reserved());
        hash_bool(
            &mut hasher,
            open_orders.native_base_total() == 0
                && open_orders.native_quote_total() == 0
                && open_orders.native_rebates() == 0,
        );
    }
    for (i, openbook_orders) in account.active_openbook_v2_orders().enumerate() {
        let base_token_index = { openbook_orders.base_token_index };
        let quote_token_index = { openbook_orders.quote_token_index };
        if !included_token_indices.contains(&base_token_index)
            || !included_token_indices.contains(&quote_token_index)
        {
            continue;
        }
        let open_orders = retriever.openbook_oo(i, &{ openbook_orders.open_orders })?;
        spot_count = spot_count.saturating_add(1);
        hash_u8(&mut hasher, 1);
        hash_u16(&mut hasher, { openbook_orders.market_index });
        hash_u64(&mut hasher, open_orders.position.base_free_native);
        hash_u64(&mut hasher, open_orders.position.quote_free_native);
        hash_i64(
            &mut hasher,
            open_orders.position.asks_base_lots * { openbook_orders.base_lot_size },
        );
        hash_i64(
            &mut hasher,
            open_orders.position.bids_quote_lots * { openbook_orders.quote_lot_size },
        );
        hash_bool(
            &mut hasher,
            open_orders.position.is_empty(open_orders.version),
        );
    }
    hash_u16(&mut hasher, spot_count);

    hasher.hash(b"perps");
    let mut perp_count = 0u16;
    for (i, perp_position) in account.active_perp_positions().enumerate() {
        let market_index = perp_position.market_index;
        let (perp_market, oracle_price) =
            retriever.perp_market_and_oracle_price(&account.fixed.group, i, market_index)?;
        let settle_token_index = { perp_market.settle_token_index };

        require!(
            included_token_indices.contains(&settle_token_index),
            MangoError::InvalidBank
        );

        perp_count = perp_count.saturating_add(1);
        hash_u16(&mut hasher, perp_market.perp_market_index);
        hash_u16(&mut hasher, settle_token_index);
        hash_i128(&mut hasher, oracle_price.to_bits());
        hash_i128(&mut hasher, perp_market.stable_price().to_bits());
        hash_i128(&mut hasher, perp_market.maint_base_asset_weight.to_bits());
        hash_i128(&mut hasher, perp_market.init_base_asset_weight.to_bits());
        hash_i128(&mut hasher, perp_market.maint_base_liab_weight.to_bits());
        hash_i128(&mut hasher, perp_market.init_base_liab_weight.to_bits());
        hash_i128(&mut hasher, perp_market.maint_overall_asset_weight.to_bits());
        hash_i128(&mut hasher, perp_market.init_overall_asset_weight.to_bits());
        hash_i64(&mut hasher, perp_market.base_lot_size);
        hash_i64(&mut hasher, perp_market.quote_lot_size);
        hash_i128(&mut hasher, perp_market.long_funding.to_bits());
        hash_i128(&mut hasher, perp_market.short_funding.to_bits());
    }
    hash_u16(&mut hasher, perp_count);

    Ok(hasher.result().to_bytes())
}

pub fn required_risk_sidecar_snapshot_capacity(account: &MangoAccountRef) -> usize {
    let token_count = account.all_token_positions().count();
    let max_spot_infos = account.all_serum3_orders().count() + account.all_openbook_v2_orders().count();
    let perp_count = account.all_perp_positions().count();
    SERIALIZED_SNAPSHOT_PREFIX_BYTES
        + token_count * SERIALIZED_TOKEN_INFO_BYTES
        + max_spot_infos * SERIALIZED_SPOT_INFO_BYTES
        + perp_count * SERIALIZED_PERP_INFO_BYTES
}

pub fn load_risk_sidecar_account<'info>(
    group: Pubkey,
    mango_account: Pubkey,
    sidecar_ai_opt: Option<&AccountInfo<'info>>,
) -> Result<Option<Account<'info, RiskSidecar>>> {
    let Some(sidecar_ai) = sidecar_ai_opt else {
        return Ok(None);
    };
    let (expected_key, _) = RiskSidecar::pda(&group, &mango_account);
    require_keys_eq!(*sidecar_ai.key, expected_key);
    require_keys_eq!(*sidecar_ai.owner, crate::id());
    Ok(Some(Account::<RiskSidecar>::try_from(sidecar_ai)?))
}

pub fn matching_risk_sidecar_snapshot(
    sidecar: &Account<RiskSidecar>,
    group: Pubkey,
    mango_account: Pubkey,
    account_state_hash: [u8; 32],
    health_accounts_state_hash: [u8; 32],
) -> Result<Option<ExactRiskSidecarSnapshot>> {
    if !sidecar.matches_state(
        group,
        mango_account,
        account_state_hash,
        health_accounts_state_hash,
    ) {
        return Ok(None);
    }

    let meta = ExactRiskSidecarMeta {
        group: sidecar.group,
        mango_account: sidecar.mango_account,
        account_sequence_number: sidecar.account_sequence_number,
        oracle_slot: sidecar.oracle_slot,
        token_count: 0,
        spot_count: 0,
        perp_count: 0,
    };

    let snapshot = ExactRiskSidecarSnapshot::decode(
        meta,
        I80F48::from_bits(sidecar.init_health_bits),
        I80F48::from_bits(sidecar.maint_health_bits),
        I80F48::from_bits(sidecar.liquidation_end_health_bits),
        &sidecar.snapshot_data,
    )?;
    if !snapshot.matches_health_cache() {
        return err!(MangoError::RiskSidecarSnapshotDecodeFailed);
    }
    Ok(Some(snapshot))
}

pub fn refresh_risk_sidecar_account(
    sidecar: &mut Account<RiskSidecar>,
    snapshot: &ExactRiskSidecarSnapshot,
    account_state_hash: [u8; 32],
    health_accounts_state_hash: [u8; 32],
    refresh_slot: u64,
    refresh_ts: u64,
    strict_capacity: bool,
) -> Result<bool> {
    let encoded = snapshot.encode()?;
    let available_capacity = RiskSidecar::snapshot_capacity(sidecar.to_account_info().data_len());
    if encoded.len() > available_capacity {
        if strict_capacity {
            return err!(MangoError::RiskSidecarCapacityTooSmall);
        }
        return Ok(false);
    }

    sidecar.version = RiskSidecar::VERSION;
    sidecar.group = snapshot.meta.group;
    sidecar.mango_account = snapshot.meta.mango_account;
    sidecar.account_sequence_number = snapshot.meta.account_sequence_number;
    sidecar.account_state_hash = account_state_hash;
    sidecar.health_accounts_state_hash = health_accounts_state_hash;
    sidecar.oracle_slot = snapshot.meta.oracle_slot;
    sidecar.last_refresh_slot = refresh_slot;
    sidecar.last_refresh_ts = refresh_ts;
    sidecar.init_health_bits = snapshot.init_health.to_bits();
    sidecar.maint_health_bits = snapshot.maint_health.to_bits();
    sidecar.liquidation_end_health_bits = snapshot.liquidation_end_health.to_bits();
    sidecar.snapshot_data = encoded;
    Ok(true)
}

impl HealthCache {
    pub fn exact_risk_sidecar_snapshot(
        &self,
        group: Pubkey,
        mango_account: Pubkey,
        account: &MangoAccountRef,
        oracle_slot: u64,
    ) -> ExactRiskSidecarSnapshot {
        ExactRiskSidecarSnapshot::rebuild(group, mango_account, account, self, oracle_slot)
    }
}

fn hash_pubkey(hasher: &mut Hasher, value: &Pubkey) {
    hasher.hash(value.as_ref());
}

fn hash_u8(hasher: &mut Hasher, value: u8) {
    hasher.hash(&[value]);
}

fn hash_bool(hasher: &mut Hasher, value: bool) {
    hash_u8(hasher, u8::from(value));
}

fn hash_u16(hasher: &mut Hasher, value: u16) {
    hasher.hash(&value.to_le_bytes());
}

fn hash_u64(hasher: &mut Hasher, value: u64) {
    hasher.hash(&value.to_le_bytes());
}

fn hash_i64(hasher: &mut Hasher, value: i64) {
    hasher.hash(&value.to_le_bytes());
}

fn hash_i128(hasher: &mut Hasher, value: i128) {
    hasher.hash(&value.to_le_bytes());
}

fn hash_f64(hasher: &mut Hasher, value: f64) {
    hasher.hash(&value.to_bits().to_le_bytes());
}

fn push_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

fn push_bool(out: &mut Vec<u8>, value: bool) {
    out.push(u8::from(value));
}

fn push_u16(out: &mut Vec<u8>, value: usize) -> Result<()> {
    let value = u16::try_from(value).map_err(|_| error!(MangoError::RiskSidecarSnapshotTooLarge))?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn push_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i128(out: &mut Vec<u8>, value: i128) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn encode_prices(out: &mut Vec<u8>, prices: &Prices) {
    push_i128(out, prices.oracle.to_bits());
    push_i128(out, prices.stable.to_bits());
}

fn read_exact<'a>(cursor: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
    require!(cursor.len() >= len, MangoError::RiskSidecarSnapshotDecodeFailed);
    let (head, tail) = cursor.split_at(len);
    *cursor = tail;
    Ok(head)
}

fn read_u8(cursor: &mut &[u8]) -> Result<u8> {
    Ok(read_exact(cursor, 1)?[0])
}

fn read_bool(cursor: &mut &[u8]) -> Result<bool> {
    Ok(match read_u8(cursor)? {
        0 => false,
        1 => true,
        _ => return err!(MangoError::RiskSidecarSnapshotDecodeFailed),
    })
}

fn read_u16(cursor: &mut &[u8]) -> Result<u16> {
    let bytes = read_exact(cursor, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_i64(cursor: &mut &[u8]) -> Result<i64> {
    let bytes = read_exact(cursor, 8)?;
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    Ok(i64::from_le_bytes(array))
}

fn read_i128(cursor: &mut &[u8]) -> Result<i128> {
    let bytes = read_exact(cursor, 16)?;
    let mut array = [0u8; 16];
    array.copy_from_slice(bytes);
    Ok(i128::from_le_bytes(array))
}

fn decode_prices(cursor: &mut &[u8]) -> Result<Prices> {
    Ok(Prices {
        oracle: I80F48::from_bits(read_i128(cursor)?),
        stable: I80F48::from_bits(read_i128(cursor)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_matches_zero_health_cache() {
        let snapshot = ExactRiskSidecarSnapshot {
            meta: ExactRiskSidecarMeta {
                group: Pubkey::default(),
                mango_account: Pubkey::default(),
                account_sequence_number: 0,
                oracle_slot: 0,
                token_count: 0,
                spot_count: 0,
                perp_count: 0,
            },
            token_infos: Vec::new(),
            spot_infos: Vec::new(),
            perp_infos: Vec::new(),
            being_liquidated: false,
            init_health: I80F48::ZERO,
            maint_health: I80F48::ZERO,
            liquidation_end_health: I80F48::ZERO,
        };

        assert!(snapshot.matches_health_cache());
    }
}
