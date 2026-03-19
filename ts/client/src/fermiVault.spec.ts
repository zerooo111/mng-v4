import { expect } from 'chai';
import { Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from '@solana/web3.js';
import { TOKEN_PROGRAM_ID } from '@solana/spl-token';
import { BN } from '@coral-xyz/anchor';

/**
 * Phase 6: Fermi Vault Tests
 *
 * These tests validate the Fermi Vault program which provides:
 *   - Vault initialization with a whitelisted program
 *   - User deposits into PDA-owned token accounts
 *   - User withdrawals with balance tracking
 *   - takeTokens by whitelisted callers (e.g., the DEX program)
 *
 * IDL source: fermilabs-frontend/src/features/vault-deposit/lib/fermi_vault.ts
 * Client:     fermilabs-frontend/src/features/vault-deposit/lib/vault_client.ts
 *
 * These tests require a local validator or devnet with the vault program deployed.
 * PDA seeds:
 *   vaultState:        ["vault_state",        mint]
 *   vaultAuthority:    ["vault_authority",     vaultState]
 *   vaultTokenAccount: ["vault_token_account", vaultState]
 *   userState:         ["user_state",          vaultState, userAta]
 *
 * Error codes (from IDL):
 *   6000 NumericalOverflow
 *   6001 InvalidWhitelistedProgram
 *   6002 UserMismatch
 *   6003 InsufficientFunds
 *   6004 InvalidWhitelistedAddress
 *   6005 ApprovalFailed
 *   6006 UnauthorizedCaller
 */

// Placeholder program ID -- replace with actual deployed vault program ID
const VAULT_PROGRAM_ID = new PublicKey('11111111111111111111111111111111');

// --------------------------------------------------------------------------
// PDA derivation helpers (mirrors LiquidityVaultClient)
// --------------------------------------------------------------------------

function deriveVaultState(mint: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('vault_state'), mint.toBuffer()],
    VAULT_PROGRAM_ID,
  );
}

function deriveVaultAuthority(vaultState: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('vault_authority'), vaultState.toBuffer()],
    VAULT_PROGRAM_ID,
  );
}

function deriveVaultTokenAccount(vaultState: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('vault_token_account'), vaultState.toBuffer()],
    VAULT_PROGRAM_ID,
  );
}

function deriveUserState(
  userAta: PublicKey,
  vaultState: PublicKey,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('user_state'), vaultState.toBuffer(), userAta.toBuffer()],
    VAULT_PROGRAM_ID,
  );
}

// --------------------------------------------------------------------------
// Sanity check: PDA derivation is deterministic
// --------------------------------------------------------------------------

describe('Fermi Vault — PDA derivation sanity', () => {
  const mint = Keypair.generate().publicKey;

  it('vaultState PDA is deterministic for the same mint', () => {
    const [a] = deriveVaultState(mint);
    const [b] = deriveVaultState(mint);
    expect(a.equals(b)).to.be.true;
  });

  it('vaultAuthority PDA is deterministic for the same vaultState', () => {
    const [vaultState] = deriveVaultState(mint);
    const [a] = deriveVaultAuthority(vaultState);
    const [b] = deriveVaultAuthority(vaultState);
    expect(a.equals(b)).to.be.true;
  });

  it('vaultTokenAccount PDA is deterministic', () => {
    const [vaultState] = deriveVaultState(mint);
    const [a] = deriveVaultTokenAccount(vaultState);
    const [b] = deriveVaultTokenAccount(vaultState);
    expect(a.equals(b)).to.be.true;
  });

  it('userState PDA is deterministic', () => {
    const user = Keypair.generate().publicKey;
    const [vaultState] = deriveVaultState(mint);
    const [a] = deriveUserState(user, vaultState);
    const [b] = deriveUserState(user, vaultState);
    expect(a.equals(b)).to.be.true;
  });

  it('different mints produce different vaultState PDAs', () => {
    const mintA = Keypair.generate().publicKey;
    const mintB = Keypair.generate().publicKey;
    const [a] = deriveVaultState(mintA);
    const [b] = deriveVaultState(mintB);
    expect(a.equals(b)).to.be.false;
  });

  it('different users produce different userState PDAs', () => {
    const [vaultState] = deriveVaultState(mint);
    const userA = Keypair.generate().publicKey;
    const userB = Keypair.generate().publicKey;
    const [a] = deriveUserState(userA, vaultState);
    const [b] = deriveUserState(userB, vaultState);
    expect(a.equals(b)).to.be.false;
  });
});

// --------------------------------------------------------------------------
// 6A. Happy Path (P0)
// --------------------------------------------------------------------------

describe('Fermi Vault — 6A Happy Path', () => {
  it.skip('initialize vault — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Create a new SPL token mint
    // 2. Derive vaultState, vaultAuthority, vaultTokenAccount PDAs
    // 3. Call program.methods.initialize(whitelistedProgram) with accounts
    // 4. Fetch vaultState and assert tokenMint, whitelistedProgram, bumps are set
    // 5. Verify vaultTokenAccount exists and has authority = vaultAuthority PDA
  });

  it.skip('deposit tokens — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault (reuse setup from above)
    // 2. Create user token account and mint tokens to it
    // 3. Call program.methods.deposit(user, amount) with accounts
    // 4. Fetch userState and assert amountDeposited == amount
    // 5. Verify vaultTokenAccount balance increased by amount
    // 6. Verify user token account balance decreased by amount
  });

  it.skip('withdraw tokens — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault and deposit tokens (reuse setup)
    // 2. Call program.methods.withdraw(user, partialAmount) with accounts
    // 3. Fetch userState and assert amountDeposited decreased correctly
    // 4. Verify user token account received the withdrawn tokens
    // 5. Verify vaultTokenAccount balance decreased by partialAmount
  });

  it.skip('withdraw full balance — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault and deposit 1000 tokens
    // 2. Call program.methods.withdraw(user, 1000) — full balance
    // 3. Fetch userState and assert amountDeposited == 0
    // 4. Verify vaultTokenAccount balance is 0 (or only other users' deposits)
    // 5. Verify user token account received all 1000 tokens back
  });

  it.skip('takeTokens by whitelisted program — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault with whitelistedProgram = known keypair
    // 2. Deposit tokens as user
    // 3. Call program.methods.takeTokens(user, amount) with caller = whitelistedProgram signer
    // 4. Fetch userState and assert amountDeposited decreased
    // 5. Verify recipientTokenAccount received the tokens
    // 6. Verify vaultTokenAccount balance decreased
  });
});

// --------------------------------------------------------------------------
// 6B. Error Paths (P0)
// --------------------------------------------------------------------------

describe('Fermi Vault — 6B Error Paths', () => {
  it.skip('withdraw more than deposited fails — InsufficientFunds (6003)', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault and deposit 100 tokens
    // 2. Attempt program.methods.withdraw(user, 200) — exceeds deposit
    // 3. Expect transaction to fail with error code 6003 (InsufficientFunds)
    // 4. Verify userState.amountDeposited is unchanged at 100
    // 5. Verify vaultTokenAccount balance is unchanged
  });

  it.skip('non-whitelisted caller fails — InvalidWhitelistedProgram (6001)', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault with whitelistedProgram = programA
    // 2. Deposit tokens as user
    // 3. Attempt takeTokens with caller = randomKeypair (not whitelisted)
    // 4. Expect transaction to fail with error code 6001 (InvalidWhitelistedProgram)
    // 5. Verify no tokens were transferred
  });

  it.skip('wrong mint fails — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault for mintA
    // 2. Create user token account for mintB
    // 3. Attempt deposit with mintB token account against mintA vault
    // 4. Expect transaction to fail (token program constraint violation)
    // 5. Verify vault state is unchanged
  });

  it.skip('double init fails — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault for mintA successfully
    // 2. Attempt to call initialize again with same mint (same PDA seeds)
    // 3. Expect transaction to fail (account already initialized / address collision)
    // 4. Verify original vault state is unchanged
  });

  it.skip('zero withdraw fails — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault and deposit tokens
    // 2. Attempt program.methods.withdraw(user, 0)
    // 3. Expect transaction to fail (zero-amount withdrawal should be rejected)
    // 4. Verify userState.amountDeposited is unchanged
  });
});

// --------------------------------------------------------------------------
// 6C. Security (P0)
// --------------------------------------------------------------------------

describe('Fermi Vault — 6C Security', () => {
  it.skip('takeTokens exceeds deposit fails — InsufficientFunds (6003)', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault and deposit 500 tokens as userA
    // 2. Call takeTokens with whitelisted caller for amount = 501
    // 3. Expect transaction to fail with error code 6003 (InsufficientFunds)
    // 4. Verify userState.amountDeposited is still 500
    // 5. Verify vaultTokenAccount balance is unchanged
  });

  it.skip('spoofed vault authority fails — requires validator', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault normally
    // 2. Deposit tokens
    // 3. Construct a withdraw instruction with a fake vaultAuthority (random keypair)
    //    instead of the real PDA
    // 4. Expect transaction to fail (PDA seeds mismatch / constraint violation)
    // 5. Verify no tokens were transferred
  });

  it.skip('withdraw to wrong account fails — UserMismatch (6002)', () => {
    // TODO: Implement with local validator or devnet
    // Steps:
    // 1. Initialize vault and deposit tokens as userA
    // 2. Construct withdraw instruction with userA's signer but userB's token account
    //    or with mismatched user pubkey in args vs. signer
    // 3. Expect transaction to fail with error code 6002 (UserMismatch)
    // 4. Verify userA's deposit is unchanged
    // 5. Verify no tokens moved to wrong account
  });
});
