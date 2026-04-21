# Fresh-deploy runbook

Record of the 2026-04-21 nuclear reset: old program id `9rpAcg1j…` was
retired and a fresh `9MkHgbHZ…` deployment was stood up with 5 perp
markets (SOL / ETH / BTC / ZEC / FARTCOIN) at indices 0–4 under a
new group (`GROUP_NUM=3`, PDA `CwUdR42yS8881q8tmesKuVko1nqsUeqmcEbcgqWVnUxf`).

Goal of this doc: if the deployment needs to be reset again, these are
the exact steps in order, including the pitfalls we hit so they can be
avoided the second time.

> **Start state:** CLUSTER_URL_OVERRIDE and MB_PAYER_KEYPAIR already in
> scope; admin keypair has upgrade authority over the current program.

## 0. Pre-flight

- Admin SOL budget: **≥ 75 SOL** transient (covers program deploy rent,
  5 perp markets, 5 per-market queues, extend fees). If you plan to
  retire an existing program in the same run, `solana program close`
  refunds ~34 SOL so the effective need is ~40 SOL.
- `enable-gpl` + `legacy-queues` Cargo features must be compiled into
  the program. `legacy-queues` is required because
  `execution_queue_v3_init_authority_state` is the only init path for
  the `ExecutionQueueAuthorityState` PDA that the v5 lifecycle
  references (v5 does not have its own init ix). Without this feature,
  the authority-state PDA can never be created and per-market queue
  bootstrap fails with 6126 `LegacyQueuesDisabled`.

## 1. Stop services (any version of the stack already running)

```sh
sudo systemctl stop stagin4-devnet-relayer.service stagin4-devnet-harness.service
```

## 2. (Optional) Retire the old program and recover rent

Irreversible — the program id can never be reused. Do this only if a
clean break from the old id is desired. Refunds ~34 SOL.

```sh
solana --url "$RPC" program close <OLD_PROGRAM_ID> \
  --bypass-warning \
  --keypair $ADMIN_KP \
  --recipient $ADMIN_KP
```

## 3. Generate a new program keypair and update references

```sh
solana-keygen new --no-bip39-passphrase --silent \
  -o target/deploy/mango_v4-keypair.json.new
NEW_PROGRAM=$(solana-keygen pubkey target/deploy/mango_v4-keypair.json.new)
mv target/deploy/mango_v4-keypair.json target/deploy/mango_v4-keypair.json.old
mv target/deploy/mango_v4-keypair.json.new target/deploy/mango_v4-keypair.json
```

Edit `declare_id!` in `programs/mango-v4/src/lib.rs`, and both
`[programs.localnet]` and `[programs.devnet]` entries in `Anchor.toml`
to the new program id. Also sweep all TS/Rust/env refs:

```sh
OLD=…; NEW=…
grep -rln --include="*.env" --include="*.ts" --include="*.rs" --include="*.json" --include="*.toml" --include="*.md" \
  --exclude-dir=node_modules --exclude-dir=target --exclude-dir=.git \
  "$OLD" . | xargs sed -i "s/$OLD/$NEW/g"
```

## 4. Build program with both feature flags

```sh
cargo-build-sbf --manifest-path programs/mango-v4/Cargo.toml \
  --features enable-gpl,legacy-queues
```

~50 s cold. `.so` lands at `target/deploy/mango_v4.so` (~4.9 MB).

## 5. Deploy program

```sh
solana --url "$RPC" program deploy target/deploy/mango_v4.so \
  --program-id target/deploy/mango_v4-keypair.json \
  --keypair $ADMIN_KP
```

**Pitfall (upgrade case only)**: if you are _upgrading_ an already-deployed
program to a larger .so, deploy fails with `account data too small`.
Run `solana program extend <program-id> <additional-bytes>` first. Our
delta for legacy-queues was ~250 KB; use a round 300000.

**Pitfall (insufficient funds)**: deploy opens a transient buffer sized
to the new .so rent (~34 SOL). If the deploy fails mid-tx it leaves the
buffer orphaned **with that SOL**, plus prints a 12-word mnemonic that
is the buffer's keypair seed. Reclaim:

```sh
# Derive the buffer keypair from the printed mnemonic (bip39 no-passphrase,
# solana legacy recovery uses seed[:32] as the ed25519 seed).
python3 -c "
import hashlib
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
import base58
m = '<paste 12 words here>'
seed = hashlib.pbkdf2_hmac('sha512', m.encode(), b'mnemonic', 2048)[:32]
sk = Ed25519PrivateKey.from_private_bytes(seed)
pk = sk.public_key().public_bytes_raw()
print(base58.b58encode(pk).decode())
open('/tmp/buf-kp.json','w').write('[' + ','.join(str(b) for b in seed+pk) + ']')
"
chown $(id -un) /tmp/buf-kp.json && chmod 600 /tmp/buf-kp.json
solana program close <printed-pubkey> --keypair $ADMIN_KP --recipient $ADMIN_KP --bypass-warning
```

Then retry step 5.

## 6. Bootstrap group + USDC bank + 5 perp markets

```sh
CLUSTER_URL_OVERRIDE=… \
CTM_RELAYER_PROGRAM_ID=$NEW_PROGRAM \
MB_PAYER_KEYPAIR=$ADMIN_KP \
EXECUTION_QUEUE_GROUP_NUM=3 \
V5_SKIP_SHARED_QUEUE=true \
V5_MARKET_INDEX_SOL=0 V5_MARKET_INDEX_ETH=1 V5_MARKET_INDEX_BTC=2 \
V5_MARKET_INDEX_ZEC=3 V5_MARKET_INDEX_FARTCOIN=4 \
./node_modules/.bin/ts-node \
  ts/client/scripts/execution-queue/v5-bootstrap-3-markets.ts
```

`V5_SKIP_SHARED_QUEUE=true` skips the legacy shared-queue step. The
program now requires per-market PDA seeds, so the shared-seed code path
in this script would always 6076. Step 7 creates the per-market queues.

**Pitfalls encountered:**

1. **Confirmation race after `groupCreate`.** First call creates the
   group, then `getGroup` immediately after can hit
   `Account does not exist` at confirmed commitment. Re-run the script;
   it's idempotent and will see the group on retry.
2. **Orphan USDC mint on partial-run resume.** If step 1 creates a mint
   but fails before the bank is registered, a later run mints a
   _second_ USDC. The group then has an insurance-mint ref to mint #1
   with no bank, and `reloadAll` fails. Patch the script to honor
   `USDC_MINT_OVERRIDE=<first-mint>` so resumes reuse the original mint
   (already applied in exp2 tree).
3. **Group with no bank is a dead end.** `getGroup` always fails for it
   (tries to decode bank-oracle prices). Start over with a fresh
   `EXECUTION_QUEUE_GROUP_NUM` (incremented by one) instead of
   attempting recovery.

## 7. Initialize the queue-authority PDA

Requires `legacy-queues` feature (step 4). Uses `init-queue-authority-state.ts`:

```sh
CLUSTER_URL_OVERRIDE=… CTM_RELAYER_PROGRAM_ID=$NEW_PROGRAM \
MB_PAYER_KEYPAIR=$ADMIN_KP EXECUTION_QUEUE_GROUP_NUM=3 \
./node_modules/.bin/ts-node \
  ts/client/scripts/execution-queue/init-queue-authority-state.ts
```

## 8. Bootstrap per-market queue PDAs

```sh
CLUSTER_URL_OVERRIDE=… CTM_RELAYER_PROGRAM_ID=$NEW_PROGRAM \
MB_PAYER_KEYPAIR=$ADMIN_KP EXECUTION_QUEUE_GROUP_NUM=3 \
V5_MARKETS=0,1,2,3,4 \
./node_modules/.bin/ts-node \
  ts/client/scripts/execution-queue/v5-bootstrap-per-market.ts
```

For each market_index: create → 10 × resize → init → configure_sub_queue.
Each market ~30 s; 5 markets ~2.5 min total. PDAs are deterministic from
(program_id, group, market_index) so the relayer derives them at runtime
— the only value of the printed PDAs is cross-checking.

## 9. Rewrite env file and restart services

`.devnet/systemd/devnet-stack.env` needs these updated atomically (old
values will derive broken PDAs or silently point at closed accounts):

- `PROGRAM_ID`, `CTM_RELAYER_PROGRAM_ID`, `CONTINUUM_HARNESS_PROGRAM_ID`
  → new program id
- `GROUP_NUM`, `EXECUTION_QUEUE_GROUP_NUM` → new group num
- `V4_GROUP`, `EXECUTION_QUEUE_GROUP_PK`, `CONTINUUM_HARNESS_GROUP_PK`
  → new group pubkey
- `V4_AUTHORITY_STATE` → new authority-state PDA
- `CONTINUUM_HARNESS_USDC_MINT` → new USDC mint
- `V5_MARKETS` → CSV of the new market indices
- `V5_M{i}_PERP_{MARKET,BIDS,ASKS,EVENT_QUEUE,ORACLE}` for each market
- **Clear** `EXECUTION_QUEUE_PK`, `EXECUTION_QUEUE_BUFFER_PK`, `V5_QUEUE`
  (obsolete under per-market topology; leaving them set makes the
  relayer look up closed accounts).
- **Clear** `V5_REVEAL_ALT_ADDRESS` (old ALT references the retired
  program id and stale market pubkeys). New ALT is a separate bootstrap
  step; until then, reveal txs fall back to legacy (non-versioned) txs.

```sh
sudo systemctl restart stagin4-devnet-relayer.service stagin4-devnet-harness.service
```

**Pitfall:** `systemctl start` on an already-active unit is a no-op. If
a stale process is running, `restart` is the only way to pick up env
edits. Verify the new PID actually inherited the expected env:

```sh
HPID=$(pgrep -fo "node.*continuum-state-harness")
sudo cat /proc/$HPID/environ | tr '\0' '\n' | grep -E "^(GROUP_NUM|V5_MARKETS|CONTINUUM_HARNESS_GROUP_PK)="
```

If env doesn't match, check both:
- `EnvironmentFile=` in the systemd drop-in — we had two, and systemd
  only resets if the drop-in begins with an empty `EnvironmentFile=`.
- The launcher scripts (`~/stagin4/scripts/run_devnet_*.sh`) re-source
  the env file _after_ their own variable assignments, so the env file
  can overwrite script-level defaults unless they re-assert after
  sourcing (we applied this fix).

## 10. Verify

```sh
# per-market queue state — expect live=0, next=0, max_seen=0 on fresh
for m in 0 1 2 3 4; do
  curl -s "http://127.0.0.1:9091/state/queue/$m?source=onchain_v5" \
    | python3 -c "import json,sys;d=json.loads(sys.stdin.read()).get('data',{});s=d.get('sub_queue') or {};print(f'm=$m pda={d.get(\"pda\",\"\")[:8]}… layout={d.get(\"layout_version\")} live={s.get(\"live_count\")} next={s.get(\"next_sequence_to_execute\")} max_seen={s.get(\"max_seen_sequence\")}')"
done

# orderbooks — expect 0 bids, 0 asks (no one has traded yet)
for m in 0 1 2 3 4; do
  curl -s "http://127.0.0.1:9091/state/book/$m?depth=2" \
    | python3 -c "import json,sys;d=json.loads(sys.stdin.read()).get('data',{});print(f'm=$m bids={len(d.get(\"bids\",[]))} asks={len(d.get(\"asks\",[]))} oracle={d.get(\"oracle_price_ui\")}')"
done
```

Pass criteria:
- All queues return `layout_version=6`, `active=true`, `live=0`, `next=0`,
  `max_seen=0`.
- All books return `bids=0`, `asks=0`, non-null `oracle_price_ui`.
- Relayer log shows no 6076 / 6122 / 6126 errors for at least 60 s under
  light bot traffic.

## 11. Out-of-scope (do later)

- **ALT rebuild**: `v5-bootstrap-alt.ts` — rebuilds the shared-reveal
  address lookup table with the new market pubkeys so reveal txs can
  batch > 2 under the versioned-tx limit.
- **Bots re-provisioning**: quoter/taker/random-taker bots hold old
  mango-account pubkeys against the retired program. They need fresh
  mango accounts under the new group; see
  `provision-quoter-bots.ts`, `provision-random-taker-bots.ts`.
- **Harness airdrop**: existing airdropped users hold SPL tokens of the
  old USDC mint, which is now useless. Fresh airdrops land them on the
  new mint.

## Record: 2026-04-21 values

```
PROGRAM_ID         = 9MkHgbHZ24xQ9YUdqUfpsakH2MBUsP8tZWtNJ4pEL8qZ
GROUP_NUM          = 3
GROUP              = CwUdR42yS8881q8tmesKuVko1nqsUeqmcEbcgqWVnUxf
AUTHORITY_STATE    = 9fXSwzrK9NmnrQ1LUcZpmkgdAqffLBRvFWaqZ3YD3Tkx
USDC_MINT          = GUdEQz4upmnxL57Xh6qKsTKU1KagpFSsgRfauuV76G7i
USDC_ORACLE        = Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX

# market_index : perp_market / bids / asks / event_queue / oracle
0 SOL-PERP     : goDzPiPa… / 8xYW6Syk… / G7W55AyJ… / FwViY7JQ… / 7UVimffx…
1 ETH-PERP     : B96Z21bM… / FJi5Hoo5… / 9DPDa5zU… / 4UuRceXv… / 42amVS4K…
2 BTC-PERP     : FNrXjqWk… / HAR1Hdr5… / G5hL1MjL… / N81RPZmx… / 4cSM2e6r…
3 ZEC-PERP     : AoQ85SrY… / CvRKNgGS… / EmSetuvV… / 3t67whui… / HzdKMXqo…
4 FARTCOIN-PERP: Ai69s48u… / 9hbLfYA4… / GivJunzY… / 4oAAKBCt… / 2t8eUbYK…

# per-market v5 queue PDAs (derivable; listed for convenience)
0: GGUVjU3iEkREsWBrbYAXUUChrshZNjqxbtSJbC5eyBNu
1: 8kVA5nsogRgNJFXMfdgbuYE5G3sJUmjBuZmkjkWwc5Yj
2: GBJWFY8VuMiNieVM4L6bwBszeeA8VFZeyohpfF3D94DH
3: GFiPdSaJP6eAPpDLuze7CaL1FsAr3TpitiHQcqmS84cq
4: AXUarLCHmA4AewXzVBz72fwgSQsiQoNBTUSh6ZCFs6Ct
```

SOL ledger cost paid out by admin during this reset:
- Close old program        : -(-34.49 SOL)  refund
- Deploy new program       : +32.39 SOL     rent
- Extend program (+300 KB) : +2.09 SOL      rent
- Bootstrap (group + bank + 5 markets + 5 queues + fees): ~ +5 SOL
- Net: roughly break-even after the old-program refund.
