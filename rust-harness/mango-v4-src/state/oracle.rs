use std::mem::size_of;

use crate::accounts_zerocopy::*;
use crate::error::*;
use crate::state::load_orca_pool_state;
use anchor_lang::prelude::*;
use anchor_lang::{AnchorDeserialize, Discriminator};
use derivative::Derivative;
use fixed::types::I80F48;
use static_assertions::const_assert_eq;

use super::{load_raydium_pool_state, orca_mainnet_whirlpool, raydium_mainnet};

/// Maximum allowed deviation between a CLMM oracle price and its fallback reference oracle.
/// 500 basis points = 5%. If the CLMM price diverges more than this from the fallback,
/// the oracle read fails with OracleConfidence error.
const CLMM_MAX_DEVIATION_FROM_REFERENCE_BPS: u64 = 500;

const DECIMAL_CONSTANT_ZERO_INDEX: i8 = 12;
const DECIMAL_CONSTANTS: [I80F48; 25] = [
    I80F48::from_bits((1 << 48) / 10i128.pow(12u32)),
    I80F48::from_bits((1 << 48) / 10i128.pow(11u32) + 1),
    I80F48::from_bits((1 << 48) / 10i128.pow(10u32)),
    I80F48::from_bits((1 << 48) / 10i128.pow(9u32) + 1),
    I80F48::from_bits((1 << 48) / 10i128.pow(8u32) + 1),
    I80F48::from_bits((1 << 48) / 10i128.pow(7u32) + 1),
    I80F48::from_bits((1 << 48) / 10i128.pow(6u32) + 1),
    I80F48::from_bits((1 << 48) / 10i128.pow(5u32)),
    I80F48::from_bits((1 << 48) / 10i128.pow(4u32)),
    I80F48::from_bits((1 << 48) / 10i128.pow(3u32) + 1), // 0.001
    I80F48::from_bits((1 << 48) / 10i128.pow(2u32) + 1), // 0.01
    I80F48::from_bits((1 << 48) / 10i128.pow(1u32) + 1), // 0.1
    I80F48::from_bits((1 << 48) * 10i128.pow(0u32)),     // 1, index 12
    I80F48::from_bits((1 << 48) * 10i128.pow(1u32)),     // 10
    I80F48::from_bits((1 << 48) * 10i128.pow(2u32)),     // 100
    I80F48::from_bits((1 << 48) * 10i128.pow(3u32)),     // 1000
    I80F48::from_bits((1 << 48) * 10i128.pow(4u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(5u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(6u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(7u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(8u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(9u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(10u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(11u32)),
    I80F48::from_bits((1 << 48) * 10i128.pow(12u32)),
];
pub const fn power_of_ten(decimals: i8) -> I80F48 {
    DECIMAL_CONSTANTS[(decimals + DECIMAL_CONSTANT_ZERO_INDEX) as usize]
}

pub const QUOTE_DECIMALS: i8 = 6;
pub const SOL_DECIMALS: i8 = 9;
pub const QUOTE_NATIVE_TO_UI: I80F48 = power_of_ten(-QUOTE_DECIMALS);

// Pyth pull-oracle (PriceUpdateV2) receiver program. Same address on mainnet,
// devnet, and all Pyth-supported SVM networks.
pub mod pyth_solana_receiver_program {
    use solana_program::declare_id;
    declare_id!("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");
}

// Sponsored PriceUpdateV2 feed accounts (shard 0). Identical on mainnet and devnet;
// the legacy `pyth_mainnet_*` naming is kept to avoid churn in downstream consumers.
pub mod pyth_mainnet_usdc_oracle {
    use solana_program::declare_id;
    declare_id!("Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX");
}

pub mod pyth_mainnet_sol_oracle {
    use solana_program::declare_id;
    declare_id!("7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE");
}

pub mod pyth_mainnet_btc_oracle {
    use solana_program::declare_id;
    declare_id!("4cSM2e6rvbGQUFiJbqytoVMi5GgghSMr8LwVrT9VPSPo");
}

pub mod pyth_mainnet_eth_oracle {
    use solana_program::declare_id;
    declare_id!("42amVS4KgzR9rA28tkVYqVXjq9Qa8dcZQMbH5EYFX6XC");
}

pub mod pyth_feed_id {
    pub const USDC_USD: [u8; 32] = [
        0xea, 0xa0, 0x20, 0xc6, 0x1c, 0xc4, 0x79, 0x71, 0x28, 0x13, 0x46, 0x1c, 0xe1, 0x53, 0x89,
        0x4a, 0x96, 0xa6, 0xc0, 0x0b, 0x21, 0xed, 0x0c, 0xfc, 0x27, 0x98, 0xd1, 0xf9, 0xa9, 0xe9,
        0xc9, 0x4a,
    ];
    pub const SOL_USD: [u8; 32] = [
        0xef, 0x0d, 0x8b, 0x6f, 0xda, 0x2c, 0xeb, 0xa4, 0x1d, 0xa1, 0x5d, 0x40, 0x95, 0xd1, 0xda,
        0x39, 0x2a, 0x0d, 0x2f, 0x8e, 0xd0, 0xc6, 0xc7, 0xbc, 0x0f, 0x4c, 0xfa, 0xc8, 0xc2, 0x80,
        0xb5, 0x6d,
    ];
    pub const BTC_USD: [u8; 32] = [
        0xe6, 0x2d, 0xf6, 0xc8, 0xb4, 0xa8, 0x5f, 0xe1, 0xa6, 0x7d, 0xb4, 0x4d, 0xc1, 0x2d, 0xe5,
        0xdb, 0x33, 0x0f, 0x7a, 0xc6, 0x6b, 0x72, 0xdc, 0x65, 0x8a, 0xfe, 0xdf, 0x0f, 0x4a, 0x41,
        0x5b, 0x43,
    ];
    pub const ETH_USD: [u8; 32] = [
        0xff, 0x61, 0x49, 0x1a, 0x93, 0x11, 0x12, 0xdd, 0xf1, 0xbd, 0x81, 0x47, 0xcd, 0x1b, 0x64,
        0x13, 0x75, 0xf7, 0x9f, 0x58, 0x25, 0x12, 0x6d, 0x66, 0x54, 0x80, 0x87, 0x46, 0x34, 0xfd,
        0x0a, 0xce,
    ];
}

// Anchor discriminator for pyth_solana_receiver_sdk::price_update::PriceUpdateV2.
// = sha256("account:PriceUpdateV2")[..8]
const PYTH_PRICE_UPDATE_V2_DISCRIMINATOR: [u8; 8] =
    [0x22, 0xf1, 0x23, 0x63, 0x9d, 0x7e, 0xf4, 0xcd];

// PriceUpdateV2 account layout (anchor-serialized):
//   [0..8)   discriminator
//   [8..40)  write_authority (Pubkey)
//   [40]     verification_level tag (0 = Partial{num_signatures:u8}, 1 = Full)
//            Partial adds 1 extra byte (num_signatures); Full adds none.
//   price_message (84 bytes): feed_id[32] | price i64 | conf u64 | exponent i32
//                             | publish_time i64 | prev_publish_time i64
//                             | ema_price i64 | ema_conf u64
//   posted_slot u64
const PYTH_V2_VERIFICATION_TAG_OFFSET: usize = 40;

pub mod usdc_mint_mainnet {
    use solana_program::declare_id;
    declare_id!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
}

pub mod sol_mint_mainnet {
    use solana_program::declare_id;
    declare_id!("So11111111111111111111111111111111111111112");
}

#[zero_copy]
#[derive(AnchorDeserialize, AnchorSerialize, Derivative, PartialEq, Eq)]
#[derivative(Debug)]
pub struct OracleConfig {
    pub conf_filter: I80F48,
    pub max_staleness_slots: i64,
    #[derivative(Debug = "ignore")]
    pub reserved: [u8; 72],
}
const_assert_eq!(size_of::<OracleConfig>(), 16 + 8 + 72);
const_assert_eq!(size_of::<OracleConfig>(), 96);
const_assert_eq!(size_of::<OracleConfig>() % 8, 0);

#[derive(AnchorDeserialize, AnchorSerialize, Debug, Default)]
pub struct OracleConfigParams {
    pub conf_filter: f32,
    pub max_staleness_slots: Option<u32>,
}

impl OracleConfigParams {
    pub fn to_oracle_config(&self) -> OracleConfig {
        OracleConfig {
            conf_filter: I80F48::from_num(self.conf_filter),
            max_staleness_slots: self.max_staleness_slots.map(|v| v as i64).unwrap_or(-1),
            reserved: [0; 72],
        }
    }
}

#[derive(Clone, Copy, PartialEq, AnchorSerialize, AnchorDeserialize)]
pub enum OracleType {
    Pyth,
    Stub,
    OrcaCLMM,
    RaydiumCLMM,
}

pub struct OracleState {
    pub price: I80F48,
    pub deviation: I80F48,
    pub last_update_slot: u64,
    pub oracle_type: OracleType,
}

impl OracleState {
    #[inline]
    pub fn check_confidence_and_maybe_staleness(
        &self,
        config: &OracleConfig,
        staleness_slot: Option<u64>,
    ) -> Result<()> {
        if let Some(now_slot) = staleness_slot {
            self.check_staleness(config, now_slot)?;
        }
        self.check_confidence(config)
    }

    pub fn check_staleness(&self, config: &OracleConfig, now_slot: u64) -> Result<()> {
        if config.max_staleness_slots >= 0
            && self
                .last_update_slot
                .saturating_add(config.max_staleness_slots as u64)
                < now_slot
        {
            return Err(MangoError::OracleStale.into());
        }
        Ok(())
    }

    pub fn check_confidence(&self, config: &OracleConfig) -> Result<()> {
        if self.deviation > config.conf_filter * self.price {
            return Err(MangoError::OracleConfidence.into());
        }
        Ok(())
    }
}

#[repr(C, packed)]
#[account(zero_copy)]
pub struct StubOracle {
    // ABI: Clients rely on this being at offset 8
    pub group: Pubkey,
    // ABI: Clients rely on this being at offset 40
    pub mint: Pubkey,
    pub price: I80F48,
    pub last_update_ts: i64,
    pub last_update_slot: u64,
    pub deviation: I80F48,
    pub reserved: [u8; 104],
}
const_assert_eq!(size_of::<StubOracle>(), 32 + 32 + 16 + 8 + 8 + 16 + 104);
const_assert_eq!(size_of::<StubOracle>(), 216);
const_assert_eq!(size_of::<StubOracle>() % 8, 0);

pub fn determine_oracle_type(acc_info: &impl KeyedAccountReader) -> Result<OracleType> {
    let data = acc_info.data();

    if acc_info.owner() == &pyth_solana_receiver_program::ID
        && data.len() >= 8
        && data[0..8] == PYTH_PRICE_UPDATE_V2_DISCRIMINATOR
    {
        return Ok(OracleType::Pyth);
    } else if data.len() >= 8 && data[0..8] == StubOracle::discriminator() {
        return Ok(OracleType::Stub);
    } else if acc_info.owner() == &orca_mainnet_whirlpool::ID {
        return Ok(OracleType::OrcaCLMM);
    } else if acc_info.owner() == &raydium_mainnet::ID {
        return Ok(OracleType::RaydiumCLMM);
    }

    Err(MangoError::UnknownOracleType.into())
}

pub fn check_is_valid_fallback_oracle(acc_info: &impl KeyedAccountReader) -> Result<()> {
    if acc_info.key() == &Pubkey::default() {
        return Ok(());
    };
    let oracle_type = determine_oracle_type(acc_info)?;
    let valid_oracle = match oracle_type {
        OracleType::OrcaCLMM => {
            let whirlpool = load_orca_pool_state(acc_info)?;
            whirlpool.has_quote_token()
        }
        OracleType::RaydiumCLMM => {
            let pool = load_raydium_pool_state(acc_info)?;
            pool.has_quote_token()
        }
        _ => true,
    };

    require!(valid_oracle, MangoError::UnexpectedOracle);
    Ok(())
}

/// Decoded fields from a Pyth pull-oracle `PriceUpdateV2` account.
/// The account layout is described next to `PYTH_V2_VERIFICATION_TAG_OFFSET`.
struct PythV2PriceFields {
    price: i64,
    conf: u64,
    exponent: i32,
    posted_slot: u64,
}

fn decode_pyth_v2(data: &[u8]) -> Result<PythV2PriceFields> {
    require!(
        data.len() >= PYTH_V2_VERIFICATION_TAG_OFFSET + 1,
        MangoError::UnknownOracleType
    );
    let tag = data[PYTH_V2_VERIFICATION_TAG_OFFSET];
    let mut off = match tag {
        1 => PYTH_V2_VERIFICATION_TAG_OFFSET + 1,     // Full: 1-byte tag only
        0 => PYTH_V2_VERIFICATION_TAG_OFFSET + 1 + 1, // Partial: tag + num_signatures
        _ => return Err(MangoError::UnknownOracleType.into()),
    };
    // price_message: feed_id[32] | price i64 | conf u64 | expo i32 | publish_time i64
    //               | prev_publish_time i64 | ema_price i64 | ema_conf u64
    // then posted_slot u64
    require!(data.len() >= off + 32 + 84 + 8, MangoError::UnknownOracleType);
    off += 32; // skip feed_id
    let price = i64::from_le_bytes(data[off..off + 8].try_into().unwrap());
    off += 8;
    let conf = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
    off += 8;
    let exponent = i32::from_le_bytes(data[off..off + 4].try_into().unwrap());
    off += 4;
    // skip publish_time, prev_publish_time, ema_price, ema_conf (8 * 4 = 32 bytes)
    off += 32;
    let posted_slot = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
    Ok(PythV2PriceFields {
        price,
        conf,
        exponent,
        posted_slot,
    })
}

pub fn get_pyth_state(
    acc_info: &(impl KeyedAccountReader + ?Sized),
    base_decimals: u8,
) -> Result<OracleState> {
    let data = acc_info.data();
    let fields = decode_pyth_v2(data)?;

    let decimals = (fields.exponent as i8) + QUOTE_DECIMALS - (base_decimals as i8);
    let decimal_adj = power_of_ten(decimals);
    let price = I80F48::from_num(fields.price) * decimal_adj;
    let deviation = I80F48::from_num(fields.conf) * decimal_adj;
    require_gte!(price, 0);
    Ok(OracleState {
        price,
        last_update_slot: fields.posted_slot,
        deviation,
        oracle_type: OracleType::Pyth,
    })
}

/// Contains all oracle account infos that could be used to read price
pub struct OracleAccountInfos<'a, T: KeyedAccountReader> {
    pub oracle: &'a T,
    pub fallback_opt: Option<&'a T>,
    pub usdc_opt: Option<&'a T>,
    pub sol_opt: Option<&'a T>,
}

impl<'a, T: KeyedAccountReader> OracleAccountInfos<'a, T> {
    pub fn from_reader(acc_reader: &'a T) -> Self {
        OracleAccountInfos {
            oracle: acc_reader,
            fallback_opt: None,
            usdc_opt: None,
            sol_opt: None,
        }
    }
}

/// Returns the price of one native base token, in native quote tokens
///
/// Example: The price for SOL at 40 USDC/SOL it would return 0.04 (the unit is USDC-native/SOL-native)
///
/// This currently assumes that quote decimals (i.e. decimals for USD) is 6, like for USDC.
///
/// The staleness and confidence of the oracle is not checked. Use the functions on
/// OracleState to validate them if needed. That's why this function is called _unchecked.
pub fn oracle_state_unchecked<T: KeyedAccountReader>(
    acc_infos: &OracleAccountInfos<T>,
    base_decimals: u8,
) -> Result<OracleState> {
    oracle_state_unchecked_inner(acc_infos, base_decimals, false)
}

pub fn fallback_oracle_state_unchecked<T: KeyedAccountReader>(
    acc_infos: &OracleAccountInfos<T>,
    base_decimals: u8,
) -> Result<OracleState> {
    oracle_state_unchecked_inner(acc_infos, base_decimals, true)
}

/// H-7 fix: Cross-validate a CLMM-derived price against a fallback reference oracle.
/// Returns the effective deviation (max of quote deviation and actual divergence).
/// Fails with OracleConfidence if the CLMM price diverges beyond the allowed band.
fn validate_clmm_against_fallback<T: KeyedAccountReader>(
    clmm_price: I80F48,
    quote_deviation: I80F48,
    acc_infos: &OracleAccountInfos<T>,
    base_decimals: u8,
) -> Result<I80F48> {
    let Some(fallback) = acc_infos.fallback_opt else {
        return Ok(quote_deviation);
    };
    if fallback.key() == &Pubkey::default() {
        return Ok(quote_deviation);
    }

    let fallback_type = determine_oracle_type(fallback)?;
    // Only cross-validate against Pyth or Stub oracles (not other CLMMs)
    let ref_state = match fallback_type {
        OracleType::Pyth => get_pyth_state(fallback, base_decimals)?,
        OracleType::Stub => {
            let stub = fallback.load::<StubOracle>()?;
            OracleState {
                price: stub.price,
                last_update_slot: if stub.last_update_slot == 0 {
                    u64::MAX
                } else {
                    stub.last_update_slot
                },
                deviation: if { stub.deviation } == 0 {
                    I80F48::MIN
                } else {
                    {
                        stub.deviation
                    }
                },
                oracle_type: OracleType::Stub,
            }
        }
        _ => return Ok(quote_deviation), // Fallback is also CLMM — skip cross-validation
    };

    if ref_state.price <= I80F48::ZERO {
        return Ok(quote_deviation);
    }

    let price_diff = (clmm_price - ref_state.price).abs();
    let max_dev = ref_state.price * I80F48::from_num(CLMM_MAX_DEVIATION_FROM_REFERENCE_BPS)
        / I80F48::from_num(10_000u64);
    require!(price_diff <= max_dev, MangoError::OracleConfidence);

    Ok(price_diff.max(quote_deviation))
}

fn oracle_state_unchecked_inner<T: KeyedAccountReader>(
    acc_infos: &OracleAccountInfos<T>,
    base_decimals: u8,
    use_fallback: bool,
) -> Result<OracleState> {
    let oracle_info = if use_fallback {
        acc_infos
            .fallback_opt
            .ok_or_else(|| error!(MangoError::UnknownOracleType))?
    } else {
        acc_infos.oracle
    };
    let data = &oracle_info.data();
    let oracle_type = determine_oracle_type(oracle_info)?;

    Ok(match oracle_type {
        OracleType::Stub => {
            let stub = oracle_info.load::<StubOracle>()?;
            let deviation = if { stub.deviation } == 0 {
                // allows the confidence check to pass even for negative prices
                I80F48::MIN
            } else {
                {
                    stub.deviation
                }
            };
            let last_update_slot = if stub.last_update_slot == 0 {
                // ensure staleness checks will never fail
                u64::MAX
            } else {
                stub.last_update_slot
            };
            OracleState {
                price: stub.price,
                last_update_slot,
                deviation,
                oracle_type: OracleType::Stub,
            }
        }
        OracleType::Pyth => get_pyth_state(oracle_info, base_decimals)?,
        OracleType::OrcaCLMM => {
            let whirlpool = load_orca_pool_state(oracle_info)?;
            let clmm_price = whirlpool.get_clmm_price();
            let quote_oracle_state = whirlpool.quote_state_unchecked(acc_infos)?;
            let price = clmm_price * quote_oracle_state.price;
            // H-7 fix: Cross-validate CLMM price against fallback to prevent manipulation
            let deviation = validate_clmm_against_fallback(
                price,
                quote_oracle_state.deviation,
                acc_infos,
                base_decimals,
            )?;
            OracleState {
                price,
                last_update_slot: quote_oracle_state.last_update_slot,
                deviation,
                oracle_type: OracleType::OrcaCLMM,
            }
        }
        OracleType::RaydiumCLMM => {
            let whirlpool = load_raydium_pool_state(oracle_info)?;
            let clmm_price = whirlpool.get_clmm_price();
            let quote_oracle_state = whirlpool.quote_state_unchecked(acc_infos)?;
            let price = clmm_price * quote_oracle_state.price;
            // H-7 fix: Cross-validate CLMM price against fallback to prevent manipulation
            let deviation = validate_clmm_against_fallback(
                price,
                quote_oracle_state.deviation,
                acc_infos,
                base_decimals,
            )?;
            OracleState {
                price,
                last_update_slot: quote_oracle_state.last_update_slot,
                deviation,
                oracle_type: OracleType::RaydiumCLMM,
            }
        }
    })
}

pub fn oracle_log_context(
    name: &str,
    state: &OracleState,
    oracle_config: &OracleConfig,
    staleness_slot: Option<u64>,
) -> String {
    format!(
        "name: {}, price: {}, deviation: {}, last_update_slot: {}, now_slot: {}, conf_filter: {:#?}",
        name,
        state.price.to_num::<f64>(),
        state.deviation.to_num::<f64>(),
        state.last_update_slot,
        staleness_slot.unwrap_or_else(|| u64::MAX),
        oracle_config.conf_filter.to_num::<f32>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program_test::{find_file, read_file};
    use std::{cell::RefCell, path::PathBuf, str::FromStr};

    #[test]
    #[ignore = "fixtures are legacy Pyth v1 PriceAccount dumps; replaced by PriceUpdateV2"]
    pub fn test_oracles() -> Result<()> {
        let fixtures = vec![
            (
                "J83w4HKfqxwcq3BEMMkPFSppX3gqekLyLJBexebFVkix",
                OracleType::Pyth,
                Pubkey::default(),
            ),
            (
                "83v8iPyZihDEjDdY8RdZddyZNyUtXngz69Lgo9Kt5d6d",
                OracleType::OrcaCLMM,
                orca_mainnet_whirlpool::ID,
            ),
            (
                "Ds33rQ1d4AXwxqyeXX6Pc3G4pFNr6iWb3dd8YfBBQMPr",
                OracleType::RaydiumCLMM,
                raydium_mainnet::ID,
            ),
        ];

        for fixture in fixtures {
            let filename = format!("resources/test/{}.bin", fixture.0);
            let mut pyth_price_data = read_file(find_file(&filename).unwrap());
            let data = RefCell::new(&mut pyth_price_data[..]);
            let ai = &AccountInfoRef {
                key: &Pubkey::from_str(fixture.0).unwrap(),
                owner: &fixture.2,
                data: data.borrow(),
            };
            assert!(determine_oracle_type(ai).unwrap() == fixture.1);
        }

        Ok(())
    }

    #[test]
    pub fn lookup_test() {
        for idx in -12..0 {
            assert_eq!(
                power_of_ten(idx),
                I80F48::from_str(&format!(
                    "0.{}1",
                    str::repeat("0", (idx.abs() as usize) - 1)
                ))
                .unwrap()
            )
        }

        assert_eq!(power_of_ten(0), I80F48::ONE);

        for idx in 1..=12 {
            assert_eq!(
                power_of_ten(idx),
                I80F48::from_str(&format!("1{}", str::repeat("0", idx.abs() as usize))).unwrap()
            )
        }
    }

    #[test]
    #[ignore = "fixtures reference legacy Pyth v1 USDC oracle; replaced by PriceUpdateV2"]
    pub fn test_clmm_prices() -> Result<()> {
        let usdc_fixture = (
            "Gnt27xtC473ZT2Mw5u8wZ68Z3gULkSTb5DuxJy7eJotD",
            OracleType::Pyth,
            Pubkey::default(),
            6,
        );

        let clmm_fixtures = vec![
            (
                "83v8iPyZihDEjDdY8RdZddyZNyUtXngz69Lgo9Kt5d6d",
                OracleType::OrcaCLMM,
                orca_mainnet_whirlpool::ID,
                9, // SOL/USDC pool
            ),
            (
                "Ds33rQ1d4AXwxqyeXX6Pc3G4pFNr6iWb3dd8YfBBQMPr",
                OracleType::RaydiumCLMM,
                raydium_mainnet::ID,
                9, // SOL/USDC pool
            ),
        ];

        for fixture in clmm_fixtures {
            let clmm_file = format!("resources/test/{}.bin", fixture.0);
            let mut clmm_data = read_file(find_file(&clmm_file).unwrap());
            let data = RefCell::new(&mut clmm_data[..]);
            let ai = &AccountInfoRef {
                key: &Pubkey::from_str(fixture.0).unwrap(),
                owner: &fixture.2,
                data: data.borrow(),
            };

            let pyth_file = format!("resources/test/{}.bin", usdc_fixture.0);
            let mut pyth_data = read_file(find_file(&pyth_file).unwrap());
            let pyth_data_cell = RefCell::new(&mut pyth_data[..]);
            let usdc_ai = &AccountInfoRef {
                key: &Pubkey::from_str(usdc_fixture.0).unwrap(),
                owner: &usdc_fixture.2,
                data: pyth_data_cell.borrow(),
            };
            let base_decimals = fixture.3;
            let usdc_decimals = usdc_fixture.3;

            let usdc_ais = OracleAccountInfos {
                oracle: usdc_ai,
                fallback_opt: None,
                usdc_opt: None,
                sol_opt: None,
            };
            let clmm_ais = OracleAccountInfos {
                oracle: ai,
                fallback_opt: None,
                usdc_opt: Some(usdc_ai),
                sol_opt: None,
            };
            let usdc = oracle_state_unchecked(&usdc_ais, usdc_decimals).unwrap();
            let clmm = oracle_state_unchecked(&clmm_ais, base_decimals).unwrap();
            assert!(usdc.price == I80F48::from_num(1.00000758274099));

            match fixture.1 {
                OracleType::OrcaCLMM => {
                    // 63.006792786538313 * 1.00000758274099 (but in native/native)
                    assert!(clmm.price == I80F48::from_num(0.06300727055072872))
                }
                OracleType::RaydiumCLMM => {
                    // 83.551469620431 * 1.00000758274099 (but in native/native)
                    assert!(clmm.price == I80F48::from_num(0.083552103169584))
                }
                _ => unimplemented!(),
            }
        }
        Ok(())
    }

    #[test]
    pub fn test_clmm_price_missing_usdc() -> Result<()> {
        // add ability to find fixtures
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("resources/test");

        let fixtures = vec![
            (
                "83v8iPyZihDEjDdY8RdZddyZNyUtXngz69Lgo9Kt5d6d",
                OracleType::OrcaCLMM,
                orca_mainnet_whirlpool::ID,
                9, // SOL/USDC pool
            ),
            (
                "Ds33rQ1d4AXwxqyeXX6Pc3G4pFNr6iWb3dd8YfBBQMPr",
                OracleType::RaydiumCLMM,
                raydium_mainnet::ID,
                9, // SOL/USDC pool
            ),
        ];

        for fixture in fixtures {
            let filename = format!("resources/test/{}.bin", fixture.0);
            let mut clmm_data = read_file(find_file(&filename).unwrap());
            let data = RefCell::new(&mut clmm_data[..]);
            let ai = &AccountInfoRef {
                key: &Pubkey::from_str(fixture.0).unwrap(),
                owner: &fixture.2,
                data: data.borrow(),
            };
            let base_decimals = fixture.3;
            assert!(determine_oracle_type(ai).unwrap() == fixture.1);
            let oracle_infos = OracleAccountInfos {
                oracle: ai,
                fallback_opt: None,
                usdc_opt: None,
                sol_opt: None,
            };
            assert!(oracle_state_unchecked(&oracle_infos, base_decimals)
                .is_anchor_error_with_code(6068));
        }

        Ok(())
    }

    #[test]
    #[ignore = "fixture USDC pyth oracle is legacy v1; replaced by PriceUpdateV2"]
    pub fn test_valid_fallbacks() -> Result<()> {
        // add ability to find fixtures
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("resources/test");

        let usdc_fixture = (
            "Gnt27xtC473ZT2Mw5u8wZ68Z3gULkSTb5DuxJy7eJotD",
            OracleType::Pyth,
            Pubkey::default(),
            6,
        );

        let clmm_fixtures = vec![
            (
                "83v8iPyZihDEjDdY8RdZddyZNyUtXngz69Lgo9Kt5d6d",
                OracleType::OrcaCLMM,
                orca_mainnet_whirlpool::ID,
                9, // SOL/USDC pool
            ),
            (
                "Ds33rQ1d4AXwxqyeXX6Pc3G4pFNr6iWb3dd8YfBBQMPr",
                OracleType::RaydiumCLMM,
                raydium_mainnet::ID,
                9, // SOL/USDC pool
            ),
        ];

        for fixture in clmm_fixtures {
            let clmm_file = format!("resources/test/{}.bin", fixture.0);
            let mut clmm_data = read_file(find_file(&clmm_file).unwrap());
            let data = RefCell::new(&mut clmm_data[..]);
            let ai = &AccountInfoRef {
                key: &Pubkey::from_str(fixture.0).unwrap(),
                owner: &fixture.2,
                data: data.borrow(),
            };

            check_is_valid_fallback_oracle(ai)?;
        }
        Ok(())
    }
}
