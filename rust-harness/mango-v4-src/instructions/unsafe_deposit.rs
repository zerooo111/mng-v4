use crate::accounts_ix::*;
use crate::accounts_zerocopy::AccountInfoRef;
use crate::error::*;
use crate::logs::*;
use crate::state::*;
use anchor_lang::prelude::*;
use fixed::types::I80F48;

/// UNSAFE TEST-ONLY PATH:
/// Credits deposits directly in protocol accounting without any token transfer.
/// Do not expose in production environments.
pub fn unsafe_deposit(ctx: Context<UnsafeDeposit>, amount: u64) -> Result<()> {
    require_msg!(amount > 0, "unsafe_deposit amount must be positive");

    let mut bank = ctx.accounts.bank.load_mut()?;
    let token_index = bank.token_index;
    let amount_i80f48 = I80F48::from(amount);

    let mut account = ctx.accounts.account.load_full_mut()?;
    let (position, _, _) = account.ensure_token_position(token_index)?;

    bank.deposit(
        position,
        amount_i80f48,
        Clock::get()?.unix_timestamp.try_into().unwrap(),
    )?;
    let indexed_position = position.indexed_position;

    // Keep net_deposits coherent with the credited amount.
    let oracle_ref = &AccountInfoRef::borrow(ctx.accounts.oracle.as_ref())?;
    let unsafe_oracle_state = oracle_state_unchecked(
        &OracleAccountInfos::from_reader(oracle_ref),
        bank.mint_decimals,
    )?;
    let amount_usd = (amount_i80f48 * unsafe_oracle_state.price).to_num::<i64>();
    account.fixed.net_deposits += amount_usd;

    if indexed_position > 0 {
        bank.check_deposit_and_oo_limit()?;
    }

    emit_stack(TokenBalanceLog {
        mango_group: ctx.accounts.group.key(),
        mango_account: ctx.accounts.account.key(),
        token_index,
        indexed_position: indexed_position.to_bits(),
        deposit_index: bank.deposit_index.to_bits(),
        borrow_index: bank.borrow_index.to_bits(),
    });

    Ok(())
}
