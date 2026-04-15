import { expect } from 'chai';
import { PublicKey } from '@solana/web3.js';
import { buildCanonicalHealthRemainingAccountKeys } from './healthAccounts';

describe('healthAccounts', () => {
  it('groups health accounts in the order expected by on-chain scanning', () => {
    const bank = new PublicKey('11111111111111111111111111111111');
    const tokenOracle = new PublicKey('11111111111111111111111111111112');
    const perpA = new PublicKey('11111111111111111111111111111113');
    const perpB = new PublicKey('11111111111111111111111111111114');
    const perpOracleA = new PublicKey('11111111111111111111111111111115');
    const perpOracleB = new PublicKey('11111111111111111111111111111116');
    const serumOo = new PublicKey('11111111111111111111111111111117');
    const fallback = new PublicKey('11111111111111111111111111111118');

    const ordered = buildCanonicalHealthRemainingAccountKeys({
      bankAccounts: [bank],
      tokenOracles: [tokenOracle],
      perpMarkets: [perpA, perpB],
      perpOracles: [perpOracleA, perpOracleB],
      serumOpenOrders: [serumOo],
      fallbackOracles: [fallback],
    });

    expect(ordered.map((key) => key.toBase58())).deep.eq([
      bank.toBase58(),
      tokenOracle.toBase58(),
      perpA.toBase58(),
      perpB.toBase58(),
      perpOracleA.toBase58(),
      perpOracleB.toBase58(),
      serumOo.toBase58(),
      fallback.toBase58(),
    ]);
  });

  it('deduplicates repeated accounts without reordering later sections', () => {
    const bank = new PublicKey('11111111111111111111111111111111');
    const tokenOracle = new PublicKey('11111111111111111111111111111112');
    const perp = new PublicKey('11111111111111111111111111111113');

    const ordered = buildCanonicalHealthRemainingAccountKeys({
      bankAccounts: [bank],
      tokenOracles: [tokenOracle, tokenOracle],
      perpMarkets: [perp],
      fallbackOracles: [tokenOracle, bank],
    });

    expect(ordered.map((key) => key.toBase58())).deep.eq([
      bank.toBase58(),
      tokenOracle.toBase58(),
      perp.toBase58(),
    ]);
  });
});
