# Batch Mint Proxy Architecture

This document describes the design and implementation of the **Batch Mint Proxy** — a separately deployable Soroban contract that batches multiple `mint_wrap` calls to the Stellar Wrap Registry **atomically**.

> **Issue:** [#517 — Add a proxy contract that can batch multiple `mint_wrap` calls atomically](https://github.com/zintarh/stellar-wrap-contract/issues/517)

## Overview

The wrap registry exposes two minting entrypoints:

- `mint_wrap` — a single wrap mint with an individual Ed25519 signature.
- `mint_wrap_batch` — in-contract batching, with an optional aggregated signature.

The Batch Mint Proxy (`batch_mint_proxy`) is a third deployment option: a **lightweight contract that forwards ordinary `mint_wrap` calls one-by-one** to a configured registry address. Because a Soroban host invocation is transactional, the proxy gets **all-or-nothing semantics for free**: if any forwarded call fails, the entire transaction — including every earlier wrap in the batch — is rolled back.

```
┌─────────────┐   invoke batch_mint_wrap(items)
│   Caller    │──────────────────────────────►┌──────────────────────┐
└─────────────┘                                │  Batch Mint Proxy     │
                                               │  (batch_mint_proxy)   │
                                               │                       │
                                               │  for each item:       │
                                               │    ┌──────────────┐   │
                                               │    │ Wrap Registry │   │
                                               │    │ mint_wrap     │◄──┼───────────────┐
                                               │    └──────────────┘   │  sub-invocation
                                               └──────────────────────┘
                                   A failing sub-call panics and rolls
                                   back the entire invocation (atomic)
```

## Why a proxy?

1. **Atomicity for individual-signature flows.** Callers that hold per-user admin signatures (rather than an aggregated batch signature) can submit many wraps in one transaction. If any item is invalid, nothing is committed.
2. **Decoupling.** The registry contract does not need new logic for third-party aggregation. The proxy is a stable integration point that forwards to whatever registry address its admin configures.
3. **Small, auditable surface.** The proxy performs no signature verification and no storage of wrap data; every security control remains in the registry.

## Contract Interface

Crate: [`proxy/`](../proxy) · Package: `batch_mint_proxy`

| Function | Description |
| --- | --- |
| `initialize(admin, wrap_contract)` | Configures the proxy admin and the target registry. Panics if already initialized. |
| `set_wrap_contract(wrap_contract)` | Re-points the proxy at a different registry. **Admin-only.** |
| `batch_mint_wrap(items) -> u32` | Forwards each `MintRequest` to the registry's `mint_wrap`, atomically. Returns the number of mints (== `items.len()`). |
| `admin() -> Option<Address>` | View: current admin. |
| `wrap_contract() -> Option<Address>` | View: configured registry address. |

### `MintRequest`

```rust
pub struct MintRequest {
    pub user: Address,
    pub period: u64,          // YYYYMM
    pub archetype: Symbol,
    pub data_hash: BytesN<32>,
    pub payload_version: u32, // must be 1 (CURRENT_PAYLOAD_VERSION)
    pub signature: BytesN<64>, // admin Ed25519 signature
}
```

The `signature` is the **registry's** admin signature over the canonical mint payload bound to the **registry's** contract address (`construct_mint_payload` with the registry address). The proxy does not alter or re-verify it; the registry validates it during each forwarded `mint_wrap`.

## Atomicity

Soroban executes a contract invocation as a single transaction:

- If the proxy's `batch_mint_wrap` succeeds, **every** item is committed.
- If **any** item fails (invalid signature, duplicate `(user, period)`, opted-out user, paused registry, expired state, etc.), the registry panics, the panic propagates through the proxy, and the host **rolls back the entire invocation** — including items that were already minted.

The proxy deliberately uses the non-catchable `invoke_contract` path (no `try_invoke_contract`), so no partial batch can ever be observed.

## Authorization & Security

### What the proxy enforces
- `initialize` is one-shot (`AlreadyInitialized` guard).
- `set_wrap_contract` requires the configured admin's authorization (`admin.require_auth()`).
- Batch size limits: empty batches are rejected (`EmptyBatch`) and batches larger than `MAX_BATCH_SIZE` (100) are rejected (`BatchTooLarge`).

### What the registry still enforces (unchanged)
- `user.require_auth()` for every mint.
- Ed25519 signature verification over the payload bound to the registry address.
- Period and payload-version validation.
- Duplicate / opt-out / pause checks.

### User authorization in a batch transaction
Because the registry calls `user.require_auth()` inside a **sub-invocation** of the proxy, a batch transaction must carry a Soroban authorization entry per user, authorizing the registry's `mint_wrap` sub-invocation with the proxy as the calling contract. Tools that assemble Soroban auth entries (or the Stellar SDK) must include these sub-invocation auths. The registry never mints for a user who has not authorized the call.

### Trust model
The registry address is set by the proxy admin. Users authorize the registry directly, so a malicious proxy cannot mint wraps the user did not sign for, nor mint without a valid admin signature. The proxy adds no privileges.

## Storage

The proxy stores two instance entries:

| Key | Value |
| --- | --- |
| `Admin` | `Address` of the proxy admin |
| `WrapContract` | `Address` of the target registry |

Instance TTL is extended to ~1 year on every write, matching the registry's conventions. The proxy stores **no wrap records** — all wrap data lives in the registry.

## Errors

| Code | Error | Trigger |
| --- | --- | --- |
| 1 | `NotInitialized` | `batch_mint_wrap` / `set_wrap_contract` before `initialize`. |
| 2 | `AlreadyInitialized` | Second `initialize` call. |
| 3 | `Unauthorized` | Non-admin calls `set_wrap_contract`. |
| 4 | `EmptyBatch` | `batch_mint_wrap` with zero items. |
| 5 | `BatchTooLarge` | More than `MAX_BATCH_SIZE` items. |

Any registry error (e.g., `WrapAlreadyExists`, `InvalidSignature`, `Paused`, `UserOptedOut`) propagates and rolls the batch back.

## Building & Deploying

```bash
# Build both contracts (registry + proxy) to WASM
make wasm-build
# or
cargo build --release --target wasm32-unknown-unknown

# Proxy WASM artifact:
#   target/wasm32-unknown-unknown/release/batch_mint_proxy.wasm

# Deploy
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/batch_mint_proxy.wasm \
  --network testnet \
  --source "$STELLAR_DEPLOYER_SECRET"

# Initialize the proxy (point it at the deployed registry)
stellar contract invoke \
  --id "$PROXY_ID" --network testnet --source "$STELLAR_DEPLOYER_SECRET" \
  -- initialize --admin "$ADMIN" --wrap_contract "$REGISTRY_ID"
```

> The proxy and the registry are separate contracts with separate WASM files. Upgrade the registry in place with `upgrade` (the proxy keeps pointing at the same address); re-point the proxy with `set_wrap_contract` only if the registry is redeployed.

## Tests

The proxy ships with a comprehensive test suite (`proxy/src/test.rs`, run via `cargo test -p batch_mint_proxy`) covering:

- Happy path: multi-user batches, single-item batches, per-item data integrity.
- **Atomicity:** invalid signature, duplicate `(user, period)`, opted-out user, and paused registry all roll back the entire batch (verified that no wrap exists afterwards).
- **Batch validation:** empty and oversized batches are rejected.
- **Initialization:** one-shot `initialize`; `batch_mint_wrap` before initialization panics.
- **Authorization:** `set_wrap_contract` requires auth, rejects non-admins, succeeds for admins, and redirects subsequent batches to the new registry.
- **Auth recording:** asserts that each user authorizes the registry's `mint_wrap` sub-invocation.

## Related Documents

- [PROXY_PATTERN_DECISION.md](../PROXY_PATTERN_DECISION.md) — why EVM-style upgradeable proxies are an anti-pattern in Soroban and why the registry uses native `upgrade` instead. The Batch Mint Proxy is a **different** kind of proxy: it does not delegate storage or code, it simply forwards calls, so it is unaffected by that decision.
- [Signing Payload](../docs/signing-payload.md) — canonical mint payload used by the registry's signature verification.
