import { PublicKey } from '@solana/web3.js';

export type HealthRemainingAccountSections = {
  bankAccounts?: PublicKey[];
  tokenOracles?: PublicKey[];
  perpMarkets?: PublicKey[];
  perpOracles?: PublicKey[];
  serumOpenOrders?: PublicKey[];
  openbookOpenOrders?: PublicKey[];
  fallbackOracles?: PublicKey[];
};

export function buildCanonicalHealthRemainingAccountKeys(
  sections: HealthRemainingAccountSections,
): PublicKey[] {
  const orderedSections: PublicKey[][] = [
    sections.bankAccounts ?? [],
    sections.tokenOracles ?? [],
    sections.perpMarkets ?? [],
    sections.perpOracles ?? [],
    sections.serumOpenOrders ?? [],
    sections.openbookOpenOrders ?? [],
    sections.fallbackOracles ?? [],
  ];

  const seen = new Set<string>();
  const orderedKeys: PublicKey[] = [];
  for (const section of orderedSections) {
    for (const key of section) {
      const keyString = key.toBase58();
      if (seen.has(keyString)) {
        continue;
      }
      seen.add(keyString);
      orderedKeys.push(key);
    }
  }

  return orderedKeys;
}
