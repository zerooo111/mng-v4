use anchor_lang::prelude::*;

#[account]
#[derive(Debug, Default)]
pub struct RiskSidecar {
    pub version: u8,
    pub bump: u8,
    pub account_sequence_number: u8,
    pub reserved: [u8; 5],
    pub group: Pubkey,
    pub mango_account: Pubkey,
    pub account_state_hash: [u8; 32],
    pub health_accounts_state_hash: [u8; 32],
    pub oracle_slot: u64,
    pub last_refresh_slot: u64,
    pub last_refresh_ts: u64,
    pub init_health_bits: i128,
    pub maint_health_bits: i128,
    pub liquidation_end_health_bits: i128,
    pub snapshot_data: Vec<u8>,
}

impl RiskSidecar {
    pub const VERSION: u8 = 1;
    const HEADER_BYTES: usize = 8
        + 8
        + 32
        + 32
        + 32
        + 32
        + 8
        + 8
        + 8
        + 16
        + 16
        + 16
        + 4;

    pub fn space(snapshot_capacity: usize) -> usize {
        8 + Self::HEADER_BYTES + snapshot_capacity
    }

    pub fn snapshot_capacity(account_data_len: usize) -> usize {
        account_data_len.saturating_sub(8 + Self::HEADER_BYTES)
    }

    pub fn pda(group: &Pubkey, mango_account: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"RiskSidecar".as_ref(), group.as_ref(), mango_account.as_ref()],
            &crate::id(),
        )
    }

    pub fn matches_state(
        &self,
        group: Pubkey,
        mango_account: Pubkey,
        account_state_hash: [u8; 32],
        health_accounts_state_hash: [u8; 32],
    ) -> bool {
        self.version == Self::VERSION
            && self.group == group
            && self.mango_account == mango_account
            && self.account_state_hash == account_state_hash
            && self.health_accounts_state_hash == health_accounts_state_hash
    }
}
